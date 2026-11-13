//! Constant-space proof of the text metrics consumed by linked-style resolution.

use super::layout::{SavedBaseline, SavedMarker, SavedTextLayout};
use crate::kiwi::FigValue;
use std::borrow::Cow;

pub(crate) fn nearly_equal(a: f64, b: f64) -> bool {
    (a - b).abs() <= 0.001
}

#[derive(Debug, Clone, PartialEq, Default)]
struct UniformMetric {
    first: Option<f64>,
    count: usize,
    invalid: bool,
}

impl UniformMetric {
    fn observe(&mut self, value: Option<f64>) {
        if self.count == 0 {
            self.first = value;
        }
        self.count += 1;
        self.invalid |= !matches!((self.first, value), (Some(first), Some(value)) if nearly_equal(first, value));
    }

    fn value(&self) -> Option<f64> {
        (!self.invalid).then_some(self.first).flatten()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Glyph {
    size: Option<f64>,
    advance: Option<f64>,
    character: Option<f64>,
    rotation: f64,
    x: Option<f64>,
    y: Option<f64>,
}

impl Glyph {
    fn from_value(value: &FigValue) -> Self {
        let position = value.get("position");
        Self {
            size: value.get_f64("fontSize"),
            advance: value.get_f64("advance"),
            character: value.get_f64("firstCharacter"),
            rotation: value.get_f64("rotation").unwrap_or(0.0),
            x: position.and_then(|p| p.get_f64("x")),
            y: position.and_then(|p| p.get_f64("y")),
        }
    }

    fn tracking_to(&self, right: &Self) -> Option<(f64, f64)> {
        if right.character? != self.character? + 1.0
            || !nearly_equal(self.rotation, 0.0)
            || !nearly_equal(right.rotation, 0.0)
            || !nearly_equal(self.y?, right.y?)
        {
            return None;
        }
        let size = self.size?;
        if size <= 0.0 || !nearly_equal(size, right.size?) {
            return None;
        }
        let tracking = right.x? - self.x? - self.advance? * size;
        (tracking.is_finite() && size.is_finite()).then_some((tracking, size))
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Tracking {
    count: usize,
    intersection: Option<(f64, f64)>,
    union: Option<(f64, f64)>,
}

impl Tracking {
    fn observe(&mut self, observed: f64, size: f64) {
        let tolerance = (size / 1024.0).max(0.05);
        let (mut low, mut high) = (observed - tolerance, observed + tolerance);
        // Rounded endpoint arithmetic can differ by one ULP from the original
        // abs(observed-candidate) comparison. Store its exact inclusive f64
        // domain instead of widening acceptance at a tolerance boundary.
        while (observed - low).abs() > tolerance {
            low = low.next_up();
        }
        while (observed - low.next_down()).abs() <= tolerance {
            low = low.next_down();
        }
        while (observed - high).abs() > tolerance {
            high = high.next_down();
        }
        while (observed - high.next_up()).abs() <= tolerance {
            high = high.next_up();
        }
        self.count += 1;
        self.intersection = Some(match self.intersection {
            Some((a, b)) => (a.max(low), b.min(high)),
            None => (low, high),
        });
        self.union = Some(match self.union {
            Some((a, b)) => (a.min(low), b.max(high)),
            None => (low, high),
        });
    }

    fn confirms(&self, current: f64, referenced: f64) -> bool {
        let (Some((low, high)), Some((union_low, union_high))) = (self.intersection, self.union)
        else {
            return false;
        };
        // A nonempty intersection means all intervals overlap, so their union
        // is contiguous. Referenced tracking must satisfy every pair, whereas
        // the stale direct tracking must satisfy none of them.
        self.count >= 2
            && low <= high
            && referenced >= low
            && referenced <= high
            && (current < union_low || current > union_high)
    }
}

/// This is a semantic summary, never a synthetic glyph/baseline array. The
/// generic Kiwi decoder and authored `textData` retain their original trees.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextEvidence {
    pub(crate) characters: Option<String>,
    glyph_size: UniformMetric,
    line_height: UniformMetric,
    font: Option<(String, String)>,
    invalid_font: bool,
    tracking: Tracking,
    pub(crate) layout: Option<Box<SavedTextLayout>>,
}

impl TextEvidence {
    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.characters.as_ref().map_or(0, String::capacity)
            + self
                .font
                .as_ref()
                .map_or(0, |(family, style)| family.capacity() + style.capacity())
            + self
                .layout
                .as_ref()
                .map_or(0, |layout| layout.retained_bytes())
    }

    pub(crate) fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        use std::hash::Hash;
        self.characters.hash(state);
        self.font.hash(state);
        self.invalid_font.hash(state);
        if let Some(layout) = &self.layout {
            layout.hash(state);
        }
        for metric in [&self.glyph_size, &self.line_height] {
            metric.first.map(f64::to_bits).hash(state);
            metric.count.hash(state);
            metric.invalid.hash(state);
        }
        self.tracking.count.hash(state);
        for range in [self.tracking.intersection, self.tracking.union] {
            range.map(|(a, b)| (a.to_bits(), b.to_bits())).hash(state);
        }
    }

    pub(crate) fn reset(&mut self, field: &str) {
        match field {
            "glyphs" => {
                self.glyph_size = UniformMetric::default();
                self.tracking = Tracking::default();
                if let Some(layout) = &mut self.layout {
                    layout.markers.clear();
                }
            }
            "baselines" => {
                self.line_height = UniformMetric::default();
                if let Some(layout) = &mut self.layout {
                    layout.baselines.clear();
                }
            }
            "fontMetaData" => {
                self.font = None;
                self.invalid_font = false;
            }
            _ => {}
        }
        if self
            .layout
            .as_ref()
            .is_some_and(|layout| layout.baselines.is_empty() && layout.markers.is_empty())
        {
            self.layout = None;
        }
    }

    pub(crate) fn from_cache(cache: &FigValue) -> Cow<'_, Self> {
        if let FigValue::TextEvidence(evidence) = cache {
            return Cow::Borrowed(evidence);
        }
        let mut evidence = Self {
            characters: cache.get_str("characters").map(str::to_owned),
            ..Self::default()
        };
        if let Some(glyphs) = cache.get_array("glyphs") {
            let mut previous = None;
            for glyph in glyphs {
                evidence.observe_glyph(glyph, &mut previous);
            }
        }
        if let Some(baselines) = cache.get_array("baselines") {
            for baseline in baselines {
                evidence.observe_baseline(baseline);
            }
        }
        if let Some(fonts) = cache.get_array("fontMetaData") {
            for font in fonts {
                evidence.observe_font(font);
            }
        }
        Cow::Owned(evidence)
    }

    pub(crate) fn metric(&self, entries: &str, field: &str) -> Option<f64> {
        match (entries, field) {
            ("glyphs", "fontSize") => self.glyph_size.value(),
            ("baselines", "lineHeight") => self.line_height.value(),
            _ => None,
        }
    }

    pub(crate) fn saved_line_count(&self) -> Option<usize> {
        self.line_height.value().map(|_| self.line_height.count)
    }

    pub(crate) fn uses_font(&self, font: &FigValue) -> bool {
        !self.invalid_font
            && matches!((&self.font, font.get_str("family"), font.get_str("style")),
                (Some((family, style)), Some(expected_family), Some(expected_style))
                    if family.eq_ignore_ascii_case(expected_family) && style.eq_ignore_ascii_case(expected_style))
    }

    pub(crate) fn confirms_tracking(&self, current: f64, referenced: f64) -> bool {
        current.is_finite()
            && referenced.is_finite()
            && !nearly_equal(current, referenced)
            && self.tracking.confirms(current, referenced)
    }

    fn observe_glyph(&mut self, value: &FigValue, previous: &mut Option<Glyph>) {
        let glyph = Glyph::from_value(value);
        self.glyph_size.observe(glyph.size);
        if let Some(marker) = SavedMarker::from_value(value) {
            self.layout
                .get_or_insert_with(Box::default)
                .markers
                .push(marker);
        }
        if let Some((observed, size)) = previous.as_ref().and_then(|left| left.tracking_to(&glyph))
        {
            self.tracking.observe(observed, size);
        }
        *previous = Some(glyph);
    }

    pub(crate) fn observe_baseline(&mut self, value: &FigValue) {
        self.line_height.observe(value.get_f64("lineHeight"));
        if let Some(baseline) = SavedBaseline::from_value(value) {
            self.layout
                .get_or_insert_with(Box::default)
                .baselines
                .push(baseline);
        }
    }

    pub(crate) fn observe_font(&mut self, value: &FigValue) {
        let Some(key) = value.get("key") else {
            self.invalid_font = true;
            return;
        };
        let (Some(family), Some(style)) = (key.get_str("family"), key.get_str("style")) else {
            self.invalid_font = true;
            return;
        };
        if let Some((existing_family, existing_style)) = &self.font {
            self.invalid_font |= !existing_family.eq_ignore_ascii_case(family)
                || !existing_style.eq_ignore_ascii_case(style);
        } else {
            self.font = Some((family.into(), style.into()));
        }
    }

    /// Stream an entire glyph array without retaining any element afterward.
    pub(crate) fn glyph_consumer(&mut self) -> impl FnMut(&FigValue) + '_ {
        let mut previous = None;
        move |glyph| self.observe_glyph(glyph, &mut previous)
    }
}

#[cfg(test)]
#[path = "evidence_tests.rs"]
mod tests;
