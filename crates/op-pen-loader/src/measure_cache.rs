//! Memoizing wrapper around a [`MeasureBackend`].
//!
//! `editor_state_to_layout_scene` rebuilds the layout scene whenever the
//! document is dirty, re-running the taffy flex solve — which calls
//! `MeasureBackend::measure` once per text leaf. Under the real skia shaper
//! (`SkiaMeasure`) each call builds and lays out a fresh `Paragraph`, the single
//! most expensive op in the layout pass. Text rarely changes between
//! reconversions (a drag / resize / colour edit leaves every string identical),
//! so memoizing on the full shaping input turns repeat measurements into hash
//! lookups. The backend lives in a thread-local that outlives individual passes,
//! so the cache spans reconversions.
//!
//! Measurement is a pure function of its inputs *for a fixed font set*, so
//! cached entries only go stale when the process-global font registry changes
//! (a runtime import/removal bumps [`jian_skia::font_generation`]). The cache
//! watches that generation and drops every entry when it advances, so an
//! already-open document re-measures with the new faces. Within a generation it
//! only grows with the set of distinct (text, font, width) tuples in play and
//! is bounded by [`MAX_ENTRIES`] and a charged byte budget.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use jian_core::layout::measure::{FontStyleKind, MeasureBackend, MeasureRequest, MeasureResult};

/// Cap on distinct cached measurements. Sized above the snapshot importer's
/// 40,000-node budget (a text-heavy web capture can carry tens of thousands
/// of distinct text leaves — at the old 8,192 such an import cleared the
/// cache mid-pass and re-measured at a near-0% hit rate); past this, drop the
/// whole cache rather than track LRU order — a full refill is one layout
/// pass, far cheaper than per-measure bookkeeping.
const MAX_ENTRIES: usize = 49_152;
/// Key bytes (including owned text/features) plus conservative table/bucket
/// overhead. Count alone cannot bound a cache of arbitrary long text nodes.
const MAX_BYTES: usize = 64 * 1024 * 1024;

/// Owned, hashable mirror of a `StyledRun`. `f32` fields are keyed by their bit
/// pattern (`to_bits`) since `f32` is not `Hash`/`Eq`; font sizes / spacings are
/// never `NaN` in practice, so distinct bit patterns map to distinct keys.
#[derive(PartialEq, Eq, Hash)]
struct RunKey {
    text: String,
    font_family: Option<String>,
    font_size_bits: u32,
    font_weight: u16,
    italic: bool,
    letter_spacing_bits: u32,
    font_features: Vec<(String, u32)>,
}

#[derive(PartialEq, Eq, Hash)]
struct MeasureKey {
    runs: Vec<RunKey>,
    line_height_bits: u32,
    max_width_bits: Option<u32>,
}

impl MeasureKey {
    fn from_request(req: &MeasureRequest<'_>) -> Self {
        let runs = req
            .runs
            .iter()
            .map(|run| RunKey {
                text: run.text.to_string(),
                font_family: run.font_family.map(str::to_string),
                font_size_bits: run.font_size.to_bits(),
                font_weight: run.font_weight,
                italic: matches!(run.font_style, FontStyleKind::Italic),
                letter_spacing_bits: run.letter_spacing.to_bits(),
                font_features: run
                    .font_features
                    .map(|features| {
                        features
                            .iter()
                            .map(|(tag, value)| (tag.clone(), *value))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect();
        MeasureKey {
            runs,
            line_height_bits: req.line_height.to_bits(),
            max_width_bits: req.max_width.map(f32::to_bits),
        }
    }
}

impl MeasureKey {
    fn matches(&self, req: &MeasureRequest<'_>) -> bool {
        self.line_height_bits == req.line_height.to_bits()
            && self.max_width_bits == req.max_width.map(f32::to_bits)
            && self.runs.len() == req.runs.len()
            && self.runs.iter().zip(req.runs).all(|(stored, run)| {
                stored.text == run.text
                    && stored.font_family.as_deref() == run.font_family
                    && stored.font_size_bits == run.font_size.to_bits()
                    && stored.font_weight == run.font_weight
                    && stored.italic == matches!(run.font_style, FontStyleKind::Italic)
                    && stored.letter_spacing_bits == run.letter_spacing.to_bits()
                    && stored.font_features.len() == run.font_features.map_or(0, |f| f.len())
                    && stored
                        .font_features
                        .iter()
                        .zip(
                            run.font_features
                                .into_iter()
                                .flat_map(|features| features.iter()),
                        )
                        .all(|((tag, value), (other, other_value))| {
                            tag == other && value == other_value
                        })
            })
    }

    fn charged_bytes(&self) -> usize {
        // 512 bytes per entry conservatively covers hash table occupancy,
        // bucket Vec spare slots and key/result handles, also on collisions.
        512usize
            .saturating_add(self.runs.capacity() * std::mem::size_of::<RunKey>())
            .saturating_add(
                self.runs
                    .iter()
                    .map(|run| {
                        run.text
                            .capacity()
                            .saturating_add(run.font_family.as_ref().map_or(0, String::capacity))
                            .saturating_add(
                                run.font_features.capacity() * std::mem::size_of::<(String, u32)>(),
                            )
                            .saturating_add(
                                run.font_features
                                    .iter()
                                    .map(|(tag, _)| tag.capacity())
                                    .sum::<usize>(),
                            )
                    })
                    .sum::<usize>(),
            )
    }
}

fn request_hash(req: &MeasureRequest<'_>) -> u64 {
    // Exhaustive shape guards: adding a shaping input requires an explicit
    // key update rather than silently creating stale hits.
    let MeasureRequest {
        runs: _,
        line_height: _,
        max_width: _,
    } = req;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    req.runs.len().hash(&mut hash);
    for run in req.runs {
        let jian_core::layout::measure::StyledRun {
            text: _,
            font_family: _,
            font_size: _,
            font_weight: _,
            font_style: _,
            letter_spacing: _,
            font_features: _,
        } = run;
        run.text.hash(&mut hash);
        run.font_family.hash(&mut hash);
        run.font_size.to_bits().hash(&mut hash);
        run.font_weight.hash(&mut hash);
        matches!(run.font_style, FontStyleKind::Italic).hash(&mut hash);
        run.letter_spacing.to_bits().hash(&mut hash);
        // Missing and explicitly empty feature maps had the same owned key.
        run.font_features
            .map_or(0, |features| features.len())
            .hash(&mut hash);
        for (tag, value) in run
            .font_features
            .into_iter()
            .flat_map(|features| features.iter())
        {
            tag.hash(&mut hash);
            value.hash(&mut hash);
        }
    }
    req.line_height.to_bits().hash(&mut hash);
    req.max_width.map(f32::to_bits).hash(&mut hash);
    hash.finish()
}

struct Entry {
    key: MeasureKey,
    result: MeasureResult,
}
// Keep the common noncolliding entry inline: borrowed-hit optimization must
// not introduce a per-key Vec allocation on the cold path. Actual hash
// collisions retain an exact-comparison bucket, never an unchecked match.
enum Bucket {
    One(Entry),
    Many(Vec<Entry>),
}
impl Bucket {
    fn find(&self, req: &MeasureRequest<'_>) -> Option<MeasureResult> {
        match self {
            Self::One(entry) => entry.key.matches(req).then_some(entry.result),
            Self::Many(entries) => entries
                .iter()
                .find(|entry| entry.key.matches(req))
                .map(|entry| entry.result),
        }
    }
    fn push(&mut self, entry: Entry) {
        if let Self::Many(entries) = self {
            entries.push(entry);
            return;
        }
        let previous = std::mem::replace(self, Self::Many(Vec::new()));
        if let Self::One(previous) = previous {
            *self = Self::Many(vec![previous, entry]);
        }
    }
}
#[derive(Default)]
struct Measurements {
    buckets: HashMap<u64, Bucket>,
    entries: usize,
    bytes: usize,
}
impl Measurements {
    fn find(&self, hash: u64, req: &MeasureRequest<'_>) -> Option<MeasureResult> {
        self.buckets.get(&hash)?.find(req)
    }
    fn insert(&mut self, hash: u64, key: MeasureKey, result: MeasureResult) {
        let bytes = key.charged_bytes();
        if bytes > MAX_BYTES {
            return;
        }
        if self.entries >= MAX_ENTRIES || bytes > MAX_BYTES.saturating_sub(self.bytes) {
            // Free table capacity along with keys; a clear must not retain a
            // previous large allocation outside the new byte charge.
            *self = Self::default();
        }
        let entry = Entry { key, result };
        match self.buckets.entry(hash) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(Bucket::One(entry));
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => slot.get_mut().push(entry),
        }
        self.entries += 1;
        self.bytes += bytes;
    }
}

/// Wraps a `MeasureBackend`, memoizing results keyed on the full shaping input.
/// `&self` measurement (Taffy hands an immutable context) uses interior
/// mutability via `RefCell`, the single-threaded pattern the trait documents.
pub struct CachingMeasureBackend {
    inner: Rc<dyn MeasureBackend>,
    cache: RefCell<Measurements>,
    built_generation: Cell<u64>,
}

impl CachingMeasureBackend {
    pub fn new(inner: Rc<dyn MeasureBackend>) -> Self {
        Self {
            inner,
            cache: RefCell::new(Measurements::default()),
            built_generation: Cell::new(crate::current_font_generation()),
        }
    }
}

impl MeasureBackend for CachingMeasureBackend {
    fn measure(&self, req: &MeasureRequest<'_>) -> MeasureResult {
        // A runtime font import/removal invalidates every memoized width.
        let generation = crate::current_font_generation();
        if generation != self.built_generation.get() {
            *self.cache.borrow_mut() = Measurements::default();
            self.built_generation.set(generation);
        }
        let hash = request_hash(req);
        if let Some(hit) = self.cache.borrow().find(hash, req) {
            return hit;
        }
        let result = self.inner.measure(req);
        // Allocate text/family/feature ownership only after a genuine miss.
        let key = MeasureKey::from_request(req);
        self.cache.borrow_mut().insert(hash, key, result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    use jian_core::layout::measure::StyledRun;

    /// Records how many times the wrapped backend was actually invoked, and
    /// returns an input-derived result so pass-through is verifiable.
    struct CountingBackend {
        calls: Cell<usize>,
    }

    impl MeasureBackend for CountingBackend {
        fn measure(&self, req: &MeasureRequest<'_>) -> MeasureResult {
            self.calls.set(self.calls.get() + 1);
            let width: f32 = req.runs.iter().map(|r| r.text.len() as f32).sum();
            MeasureResult {
                width,
                height: 4.0,
                line_count: 1,
                baseline: 3.2,
            }
        }
    }

    fn run(text: &str, size: f32, weight: u16) -> StyledRun<'_> {
        StyledRun {
            font_features: None,
            text,
            font_family: None,
            font_size: size,
            font_weight: weight,
            font_style: FontStyleKind::Normal,
            letter_spacing: 0.0,
        }
    }

    #[test]
    fn identical_requests_hit_the_cache() {
        let counter = Rc::new(CountingBackend {
            calls: Cell::new(0),
        });
        let backend = CachingMeasureBackend::new(counter.clone());

        let runs = [run("hello", 16.0, 400)];
        let req = MeasureRequest {
            runs: &runs,
            line_height: 0.0,
            max_width: None,
        };

        let first = backend.measure(&req);
        let second = backend.measure(&req);

        assert_eq!(
            counter.calls.get(),
            1,
            "second identical measure must hit the cache"
        );
        assert_eq!(first.width, second.width);
        assert_eq!(
            first.width, 5.0,
            "result must pass through from the inner backend"
        );
    }

    #[test]
    fn distinct_inputs_each_measure_once() {
        let counter = Rc::new(CountingBackend {
            calls: Cell::new(0),
        });
        let backend = CachingMeasureBackend::new(counter.clone());

        // Each axis that feeds shaping must produce a distinct cache key.
        let cases: &[(&str, f32, u16, f32, Option<f32>)] = &[
            ("hello", 16.0, 400, 0.0, None),
            ("world", 16.0, 400, 0.0, None),       // text differs
            ("hello", 18.0, 400, 0.0, None),       // size differs
            ("hello", 16.0, 700, 0.0, None),       // weight differs
            ("hello", 16.0, 400, 0.0, Some(80.0)), // max_width differs
            ("hello", 16.0, 400, 2.0, None),       // line_height differs
        ];
        for (text, size, weight, line_height, max_width) in cases {
            let runs = [run(text, *size, *weight)];
            let req = MeasureRequest {
                runs: &runs,
                line_height: *line_height,
                max_width: *max_width,
            };
            backend.measure(&req);
            // Re-measuring the same case must not add a call.
            backend.measure(&req);
        }

        assert_eq!(
            counter.calls.get(),
            cases.len(),
            "each distinct shaping input measures exactly once"
        );
    }
}

#[cfg(test)]
mod feature_cache_tests {
    use super::*;
    use jian_core::layout::measure::StyledRun;

    #[test]
    fn font_features_are_part_of_measurement_cache_key() {
        let on = std::collections::BTreeMap::from([("salt".into(), 1)]);
        let off = std::collections::BTreeMap::from([("salt".into(), 0)]);
        let key = |features| {
            let run = StyledRun {
                text: "g",
                font_family: Some("Open Sans"),
                font_size: 56.0,
                font_weight: 400,
                font_style: FontStyleKind::Normal,
                letter_spacing: 0.0,
                font_features: Some(features),
            };
            MeasureKey::from_request(&MeasureRequest {
                runs: &[run],
                line_height: 0.0,
                max_width: None,
            })
        };
        assert!(
            key(&on) != key(&off),
            "ON and OFF must not share a cached width"
        );
        assert!(
            key(&on) == key(&on),
            "identical sorted feature inputs share a key"
        );
    }
}
