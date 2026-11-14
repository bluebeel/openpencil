//! Short-lived reuse for immutable text drawn by repeated export cameras.

use skia_safe::{Data, Font, GlyphId, Matrix, Path, Point};
use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{Hash, Hasher},
    rc::Rc,
};

#[path = "vector_geometry_reuse.rs"]
mod geometry;

// Shared by these scoped entries and persistent immutable coverage masks.
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

struct GlyphEntry {
    font: Font,
    glyph: GlyphId,
    modern: bool,
    path: Path,
}

struct MeshEntry {
    original: Path,
    triangles: Rc<Vec<super::stencil::Triangle>>,
}

struct SerializedEntry {
    original: Path,
    bytes: Data,
    digest: u64,
}

/// A miss outside export scope retains the original Vec, with no additional
/// Rc allocation. Admitted immutable geometry shares that same Vec across
/// cameras; its capacity remains part of the stencil working-limit check.
pub(super) enum PreparedTriangles {
    Owned(Vec<super::stencil::Triangle>),
    Shared(Rc<Vec<super::stencil::Triangle>>),
}
impl std::ops::Deref for PreparedTriangles {
    type Target = Vec<super::stencil::Triangle>;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(triangles) => triangles,
            Self::Shared(triangles) => triangles,
        }
    }
}

struct Cache {
    generation: u64,
    outlines: HashMap<u64, Vec<OutlineEntry>>,
    flattened: HashMap<u64, Vec<FlatEntry>>,
    glyph_paths: HashMap<u64, Vec<GlyphEntry>>,
    meshes: HashMap<u32, Vec<MeshEntry>>,
    serialized: HashMap<u32, Vec<SerializedEntry>>,
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
            glyph_paths: HashMap::new(),
            meshes: HashMap::new(),
            serialized: HashMap::new(),
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
            self.glyph_paths = HashMap::new();
            self.meshes = HashMap::new();
            self.serialized = HashMap::new();
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
        let available = self.max_bytes - self.bytes - bytes;
        geometry::trim(available);
        super::stencil::trim_retained_cache(available.saturating_sub(geometry::retained_bytes()));
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

/// Glyph geometry is independent of the run's positions and paint. Retain
/// only successful outlines, so missing/unsupported paths keep their retry
/// and bounds-fallback behavior. Full Font equality checks all font settings.
pub(super) fn glyph_outline(
    font: &Font,
    glyph: GlyphId,
    modern: bool,
    build: impl FnOnce() -> Option<Path>,
) -> Option<Path> {
    if !ACTIVE.with(|slot| slot.borrow().is_some()) {
        return build();
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    font.typeface().unique_id().hash(&mut hash);
    font.size().to_bits().hash(&mut hash);
    glyph.hash(&mut hash);
    modern.hash(&mut hash);
    let hash = hash.finish();
    let cached = with_cache(|cache| {
        cache.glyph_paths.get(&hash).and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry.font == *font && entry.glyph == glyph && entry.modern == modern)
                .map(|entry| entry.path.clone())
        })
    })
    .flatten();
    if cached.is_some() {
        return cached;
    }
    let path = build()?;
    with_cache(|cache| {
        let bytes = ENTRY_OVERHEAD + 2 * path.approximate_bytes_used();
        if cache.admit(bytes) {
            cache.glyph_paths.entry(hash).or_default().push(GlyphEntry {
                font: font.clone(),
                glyph,
                modern,
                path: path.clone(),
            });
        }
    });
    Some(path)
}

/// Local triangles contain no camera/clip/color state. Equality is checked
/// against the complete immutable source path, not a revision or hash alone.
pub(super) fn triangles(
    path: &Path,
    build: impl FnOnce() -> Option<Vec<super::stencil::Triangle>>,
) -> Option<PreparedTriangles> {
    if !ACTIVE.with(|slot| slot.borrow().is_some()) {
        return build().map(PreparedTriangles::Owned);
    }
    let generation = path.generation_id();
    let cached = with_cache(|cache| {
        cache.meshes.get(&generation).and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry.original == *path)
                .map(|entry| Rc::clone(&entry.triangles))
        })
    })
    .flatten();
    if let Some(triangles) = cached {
        return Some(PreparedTriangles::Shared(triangles));
    }
    let mut owned = Some(build()?);
    let shared = with_cache(|cache| {
        let triangles = owned.as_ref()?;
        let bytes = ENTRY_OVERHEAD
            + 2 * (path.approximate_bytes_used()
                + triangles.capacity() * std::mem::size_of::<super::stencil::Triangle>());
        if !cache.admit(bytes) {
            return None;
        }
        let triangles = Rc::new(owned.take()?);
        cache.meshes.entry(generation).or_default().push(MeshEntry {
            original: path.clone(),
            triangles: Rc::clone(&triangles),
        });
        Some(triangles)
    })
    .flatten();
    match shared {
        Some(triangles) => Some(PreparedTriangles::Shared(triangles)),
        None => owned.map(PreparedTriangles::Owned),
    }
}

/// Serializing and hashing a large run path on every camera is preparation,
/// not rasterization. Share the exact serialized data/digest within this
/// export, subject to the same mask/outline/metadata byte and entry limits.
pub(super) fn serialized_path(path: &Path) -> (Data, u64) {
    let active = ACTIVE.with(|slot| slot.borrow().is_some());
    let generation = path.generation_id();
    if active {
        if let Some(cached) = with_cache(|cache| {
            cache.serialized.get(&generation).and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry.original == *path)
                    .map(|entry| (entry.bytes.clone(), entry.digest))
            })
        })
        .flatten()
        {
            return cached;
        }
    }
    let data = path.serialize();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    data.as_bytes().hash(&mut hash);
    let digest = hash.finish();
    if active {
        with_cache(|cache| {
            let bytes = ENTRY_OVERHEAD + 2 * (path.approximate_bytes_used() + data.len());
            if cache.admit(bytes) {
                cache
                    .serialized
                    .entry(generation)
                    .or_default()
                    .push(SerializedEntry {
                        original: path.clone(),
                        bytes: data.clone(),
                        digest,
                    });
            }
        });
    }
    (data, digest)
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

pub(super) fn request_headroom() -> usize {
    ACTIVE.with(|slot| {
        slot.borrow().as_ref().map_or(MAX_BYTES, |cache| {
            cache.max_bytes.saturating_sub(cache.bytes)
        })
    })
}

pub(super) fn cache_headroom() -> usize {
    request_headroom().saturating_sub(geometry::retained_bytes())
}

pub(super) fn transformed_path(path: &Path, matrix: &Matrix, even_odd: bool) -> Path {
    let build = || {
        let mut result = path.with_transform(matrix);
        if even_odd {
            result.set_fill_type(skia_safe::PathFillType::EvenOdd);
        }
        result
    };
    if !jian_skia::raster_sampling::is_point_sampling()
        || !matrix.is_finite()
        || matrix.has_perspective()
    {
        return build();
    }
    let key = geometry::Key::Transform {
        matrix: std::array::from_fn(|i| matrix[i].to_bits()),
        even_odd,
    };
    match geometry::get_or_build(path, key, || Some(geometry::Value::Path(build()))) {
        Some(geometry::Value::Path(path)) => path,
        _ => unreachable!("successful transformed path cache kind"),
    }
}
pub(super) fn fitted_path(
    path: &Path,
    bounds: [f32; 4],
    even_odd: bool,
    build: impl FnOnce() -> Path,
) -> Path {
    if !jian_skia::raster_sampling::is_point_sampling()
        || bounds.iter().any(|value| !value.is_finite())
    {
        return build();
    }
    let key = geometry::Key::Fit {
        bounds: bounds.map(f32::to_bits),
        even_odd,
    };
    match geometry::get_or_build(path, key, || Some(geometry::Value::Path(build()))) {
        Some(geometry::Value::Path(path)) => path,
        _ => unreachable!("successful fitted path cache kind"),
    }
}
pub(super) fn tight_bounds(path: &Path) -> skia_safe::Rect {
    if !jian_skia::raster_sampling::is_point_sampling() {
        return path.compute_tight_bounds();
    }
    match geometry::get_or_build(path, geometry::Key::TightBounds, || {
        Some(geometry::Value::Bounds(path.compute_tight_bounds()))
    }) {
        Some(geometry::Value::Bounds(bounds)) => bounds,
        _ => unreachable!("successful path bounds cache kind"),
    }
}
pub(super) fn stroked_path(
    path: &Path,
    paint: &skia_safe::Paint,
    matrix: &Matrix,
    build: impl FnOnce() -> Option<Path>,
) -> Option<Path> {
    let key = geometry::Key::Stroke {
        matrix: std::array::from_fn(|i| matrix[i].to_bits()),
        style: paint.style() as u32,
        width: paint.stroke_width().to_bits(),
        miter: paint.stroke_miter().to_bits(),
        cap: paint.stroke_cap() as u32,
        join: paint.stroke_join() as u32,
    };
    match geometry::get_or_build(path, key, || build().map(geometry::Value::Path))? {
        geometry::Value::Path(path) => Some(path),
        geometry::Value::Bounds(_) => unreachable!("stroke cache kind"),
    }
}

#[cfg(test)]
#[test]
fn retained_masks_and_request_entries_enforce_the_budget_in_both_directions() {
    super::stencil::trim_retained_cache(0);
    with_scope(|| {
        let mut p = skia_safe::PathBuilder::new();
        p.move_to((0., 0.));
        p.quad_to((12., -4.), (20., 12.));
        p.close();
        let p = p.detach();
        let draw = || {
            super::stencil::coverage(&p, &Matrix::default(), skia_safe::IRect::from_wh(32, 32), 2)
                .unwrap()
        };
        draw();
        let active = draw();
        assert!(super::stencil::retained_coverage_bytes() > 0);
        with_cache(|cache| {
            assert!(cache.admit(cache.max_bytes - cache.bytes));
            assert_eq!(super::stencil::retained_coverage_bytes(), 0);
        });
        drop(active);
        draw();
        assert_eq!(
            super::stencil::retained_coverage_bytes(),
            0,
            "full request cache prevents mask retention"
        );
        with_cache(|cache| {
            cache.bytes = 0;
            cache.entries = 0;
        });
        draw();
        draw();
        assert!(super::stencil::retained_coverage_bytes() > 0);
    });
    super::stencil::trim_retained_cache(0);
}
