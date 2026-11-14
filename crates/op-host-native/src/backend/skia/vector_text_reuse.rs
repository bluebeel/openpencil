//! Short-lived reuse for immutable text drawn by repeated export cameras.

use skia_safe::{Font, GlyphId, Matrix, Path, Point};
use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{Hash, Hasher},
    rc::Rc,
};

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 512;
const ENTRY_OVERHEAD: usize = 512;

thread_local! {
    static ACTIVE: RefCell<Option<Cache>> = const { RefCell::new(None) };
}

/// Nested exports share the outer budget. Unwind drops the entire cache;
/// no font/outline entry survives the owning export's return.
pub(super) fn with_scope<R>(action: impl FnOnce() -> R) -> R {
    if ACTIVE.with(|slot| slot.borrow().is_some()) {
        return action();
    }
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            ACTIVE.with(|slot| {
                slot.borrow_mut().take();
            });
        }
    }
    ACTIVE.with(|slot| *slot.borrow_mut() = Some(Cache::new(MAX_BYTES, MAX_ENTRIES)));
    let _restore = Restore;
    action()
}

struct OutlineEntry {
    font: Font,
    modern: bool,
    glyphs: Box<[GlyphId]>,
    positions: Box<[[u32; 2]]>,
    origin: [u32; 2],
    path: Path,
}

struct FlatEntry {
    original: Path,
    linear: [u32; 4],
    path: Path,
}

struct Cache {
    generation: u64,
    outlines: HashMap<u64, Vec<OutlineEntry>>,
    flattened: HashMap<u64, Vec<FlatEntry>>,
    metadata: HashMap<(u32, GlyphId), Option<Rc<super::units::GlyphMetadata>>>,
    bytes: usize,
    entries: usize,
    max_bytes: usize,
    max_entries: usize,
}

impl Cache {
    fn new(max_bytes: usize, max_entries: usize) -> Self {
        Self {
            generation: jian_skia::font_generation(),
            outlines: HashMap::new(),
            flattened: HashMap::new(),
            metadata: HashMap::new(),
            bytes: 0,
            entries: 0,
            max_bytes,
            max_entries,
        }
    }

    fn refresh(&mut self, generation: u64) {
        if self.generation != generation {
            self.outlines = HashMap::new();
            self.flattened = HashMap::new();
            self.metadata = HashMap::new();
            self.bytes = 0;
            self.entries = 0;
            self.generation = generation;
        }
    }

    fn admit(&mut self, bytes: usize) -> bool {
        if self.entries >= self.max_entries || bytes > self.max_bytes.saturating_sub(self.bytes) {
            return false;
        }
        self.entries += 1;
        self.bytes += bytes;
        true
    }
}

fn with_cache<R>(action: impl FnOnce(&mut Cache) -> R) -> Option<R> {
    ACTIVE.with(|slot| {
        let mut active = slot.borrow_mut();
        let cache = active.as_mut()?;
        cache.refresh(jian_skia::font_generation());
        Some(action(cache))
    })
}

fn point_bits(p: Point) -> [u32; 2] {
    [p.x.to_bits(), p.y.to_bits()]
}

fn outline_hash(
    font: &Font,
    glyphs: &[GlyphId],
    positions: &[Point],
    origin: Point,
    modern: bool,
) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    font.typeface().unique_id().hash(&mut hash);
    font.size().to_bits().hash(&mut hash);
    glyphs.hash(&mut hash);
    modern.hash(&mut hash);
    for p in positions {
        point_bits(*p).hash(&mut hash);
    }
    point_bits(origin).hash(&mut hash);
    hash.finish()
}

pub(super) fn outline(
    font: &Font,
    glyphs: &[GlyphId],
    positions: &[Point],
    origin: Point,
    modern: bool,
    build: impl FnOnce() -> Option<Path>,
) -> Option<Path> {
    // Avoid hashing/allocating on ordinary interactive or legacy paints.
    if !ACTIVE.with(|slot| slot.borrow().is_some()) {
        return build();
    }
    let hash = outline_hash(font, glyphs, positions, origin, modern);
    let found = with_cache(|cache| {
        cache.outlines.get(&hash).and_then(|entries| {
            entries
                .iter()
                .find(|entry| {
                    entry.font == *font
                        && entry.modern == modern
                        && entry.glyphs.as_ref() == glyphs
                        && entry.origin == point_bits(origin)
                        && entry.positions.len() == positions.len()
                        && entry
                            .positions
                            .iter()
                            .zip(positions)
                            .all(|(a, b)| *a == point_bits(*b))
                })
                .map(|entry| entry.path.clone())
        })
    })
    .flatten();
    if found.is_some() {
        return found;
    }
    // Keep font/path work outside the RefCell borrow (including fallback).
    let path = build()?;
    with_cache(|cache| {
        let bytes = ENTRY_OVERHEAD
            + glyphs.len() * std::mem::size_of::<GlyphId>()
            + positions.len() * std::mem::size_of::<[u32; 2]>()
            + 2 * path.approximate_bytes_used();
        if cache.admit(bytes) {
            cache.outlines.entry(hash).or_default().push(OutlineEntry {
                font: font.clone(),
                modern,
                glyphs: glyphs.into(),
                positions: positions.iter().map(|p| point_bits(*p)).collect(),
                origin: point_bits(origin),
                path: path.clone(),
            });
        }
    });
    Some(path)
}

pub(super) fn flattened(
    path: &Path,
    matrix: &Matrix,
    build: impl FnOnce() -> Option<Path>,
) -> Option<Path> {
    if !ACTIVE.with(|slot| slot.borrow().is_some()) {
        return build();
    }
    if !matrix.is_finite() || matrix.has_perspective() {
        return build();
    }
    // The subdivision's error norm uses only these four coefficients.
    // Camera jitter is translation-only, so it cannot change subdivision.
    let x = matrix.map_vector((1.0, 0.0));
    let y = matrix.map_vector((0.0, 1.0));
    let linear = [x.x.to_bits(), x.y.to_bits(), y.x.to_bits(), y.y.to_bits()];
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    path.generation_id().hash(&mut hash);
    linear.hash(&mut hash);
    let hash = hash.finish();
    let found = with_cache(|cache| {
        cache.flattened.get(&hash).and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry.linear == linear && entry.original == *path)
                .map(|entry| entry.path.clone())
        })
    })
    .flatten();
    if found.is_some() {
        return found;
    }
    let result = build()?;
    with_cache(|cache| {
        let bytes =
            ENTRY_OVERHEAD + 2 * (path.approximate_bytes_used() + result.approximate_bytes_used());
        if cache.admit(bytes) {
            cache.flattened.entry(hash).or_default().push(FlatEntry {
                original: path.clone(),
                linear,
                path: result.clone(),
            });
        }
    });
    Some(result)
}

#[cfg(test)]
#[path = "vector_text_reuse_tests.rs"]
mod tests;

pub(super) fn glyph_metadata(
    face: u32,
    glyph: GlyphId,
    build: impl FnOnce() -> Option<super::units::GlyphMetadata>,
) -> Option<Rc<super::units::GlyphMetadata>> {
    let key = (face, glyph);
    let mut present = false;
    let cached = with_cache(|cache| {
        if let Some(value) = cache.metadata.get(&key) {
            present = true;
            value.clone()
        } else {
            None
        }
    })
    .flatten();
    if present {
        return cached;
    }
    let metadata = build().map(Rc::new);
    with_cache(|cache| {
        let bytes = ENTRY_OVERHEAD + 2 * metadata.as_ref().map_or(0, |m| m.bytes());
        if cache.admit(bytes) {
            cache.metadata.insert(key, metadata.clone());
        }
    });
    metadata
}
