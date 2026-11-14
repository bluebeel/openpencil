//! Exact-input, render-thread paragraph reuse. Native Paragraph/font handles
//! never cross a thread or outlive the backend/font generation that owns them.
use super::super::vector_text::MetadataCache;
use super::{ShapedRun, ShapedText, NATURAL_LAYOUT_BUDGET};
use jian_skia::FontResolver;
use skia_safe::textlayout::Paragraph;
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    hash::{Hash, Hasher},
};
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENTRY: usize = 2 * 1024 * 1024;
const MAX_ENTRIES: usize = 128;
#[derive(PartialEq)]
struct Key {
    text: String,
    family: String,
    floats: [u32; 3],
    weight: u16,
    italic: bool,
    kerning: Option<bool>,
    features: Option<BTreeMap<String, u32>>,
    color: [u8; 4],
    source: Option<[u32; 4]>,
    fitted: bool,
}
fn color(run: &ShapedRun<'_>) -> [u8; 4] {
    [run.color.a(), run.color.r(), run.color.g(), run.color.b()]
}
fn floats(run: &ShapedRun<'_>) -> [u32; 3] {
    [
        run.font_size.to_bits(),
        run.letter_spacing.to_bits(),
        run.line_height.to_bits(),
    ]
}
fn source(run: &ShapedRun<'_>) -> Option<[u32; 4]> {
    run.source_color.map(|color| {
        [
            color.r.to_bits(),
            color.g.to_bits(),
            color.b.to_bits(),
            color.a.to_bits(),
        ]
    })
}
impl Key {
    fn new(run: &ShapedRun<'_>, fitted: bool) -> Self {
        Self {
            text: run.text.to_owned(),
            family: run.family.to_owned(),
            floats: floats(run),
            weight: run.weight,
            italic: run.italic,
            kerning: run.kerning,
            features: run.font_features.cloned(),
            color: color(run),
            source: source(run),
            fitted,
        }
    }
    fn matches(&self, run: &ShapedRun<'_>, fitted: bool) -> bool {
        self.text == run.text
            && self.family == run.family
            && self.floats == floats(run)
            && self.weight == run.weight
            && self.italic == run.italic
            && self.kerning == run.kerning
            && self.features.as_ref() == run.font_features
            && self.color == color(run)
            && self.source == source(run)
            && self.fitted == fitted
    }
}
fn hash(run: &ShapedRun<'_>, fitted: bool) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    run.text.hash(&mut hash);
    run.family.hash(&mut hash);
    floats(run).hash(&mut hash);
    run.weight.hash(&mut hash);
    run.italic.hash(&mut hash);
    run.kerning.hash(&mut hash);
    run.font_features.hash(&mut hash);
    color(run).hash(&mut hash);
    source(run).hash(&mut hash);
    fitted.hash(&mut hash);
    hash.finish()
}
// Paragraph's native ownership is opaque. Bound both entries and retained
// input/estimated glyph layout storage; oversize requests stay transient.
fn charge(run: &ShapedRun<'_>) -> usize {
    8192usize
        .saturating_add(run.text.len().saturating_mul(512))
        .saturating_add(run.family.len().saturating_mul(4))
        .saturating_add(run.font_features.map_or(0, |features| {
            features
                .iter()
                .map(|(tag, _)| tag.len().saturating_add(128))
                .sum::<usize>()
        }))
}
struct Entry {
    hash: u64,
    key: Key,
    paragraph: Paragraph,
    bytes: usize,
}
#[derive(Default)]
pub(super) struct Cache {
    entries: VecDeque<Entry>,
    bytes: usize,
}
impl Cache {
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
    fn take(&mut self, run: &ShapedRun<'_>, fitted: bool) -> Option<Entry> {
        let hash = hash(run, fitted);
        let at = self
            .entries
            .iter()
            .position(|entry| entry.hash == hash && entry.key.matches(run, fitted))?;
        let entry = self.entries.remove(at)?;
        self.bytes -= entry.bytes;
        Some(entry)
    }
    fn put(&mut self, entry: Entry) {
        while self.entries.len() >= MAX_ENTRIES
            || self.bytes.saturating_add(entry.bytes) > MAX_BYTES
        {
            let Some(old) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= old.bytes;
        }
        self.bytes += entry.bytes;
        self.entries.push_back(entry);
    }
}
fn layout(paragraph: &mut Paragraph, fitted: bool) {
    paragraph.layout(NATURAL_LAYOUT_BUDGET);
    if fitted {
        let natural = paragraph.max_intrinsic_width();
        paragraph.layout(natural.ceil());
    }
}
impl ShapedText {
    pub(super) fn with_paragraph<R>(
        &mut self,
        resolver: &FontResolver,
        run: &ShapedRun<'_>,
        fitted: bool,
        action: impl FnOnce(&mut Paragraph, &RefCell<MetadataCache>) -> R,
    ) -> R {
        // collection() clears old-generation paragraphs before any lookup.
        let _ = self.collection(resolver);
        let bytes = charge(run);
        if bytes > MAX_ENTRY {
            let mut paragraph = self.build(resolver, run);
            layout(&mut paragraph, fitted);
            return action(&mut paragraph, &self.metadata);
        }
        let mut entry = match self.paragraphs.take(run, fitted) {
            Some(entry) => entry,
            None => {
                let mut paragraph = self.build(resolver, run);
                layout(&mut paragraph, fitted);
                Entry {
                    hash: hash(run, fitted),
                    key: Key::new(run, fitted),
                    paragraph,
                    bytes,
                }
            }
        };
        // Move the paragraph out before callbacks, avoiding a cache borrow
        // during glyph visits. Unwind drops this entry instead of retaining
        // partially visited state; the ordinary backend/font lock unwinds too.
        let result = action(&mut entry.paragraph, &self.metadata);
        self.paragraphs.put(entry);
        result
    }
}
