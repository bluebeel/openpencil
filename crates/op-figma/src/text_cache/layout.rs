//! Small saved line records used only for Preserve-mode list geometry.

use crate::kiwi::FigValue;

pub(crate) const PRESERVED_TEXT_LAYOUT: &str = "_preservedTextLayout";

#[derive(Debug, Clone, PartialEq)]
pub struct SavedBaseline {
    pub x: f64,
    pub y: f64,
    pub line_y: f64,
    pub line_height: f64,
    pub first: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedMarker {
    pub x: f64,
    pub y: f64,
    pub font_size: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SavedTextLayout {
    pub baselines: Vec<SavedBaseline>,
    pub markers: Vec<SavedMarker>,
    pub source_size: Option<(f64, f64)>,
}

impl SavedTextLayout {
    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.baselines.capacity() * std::mem::size_of::<SavedBaseline>()
            + self.markers.capacity() * std::mem::size_of::<SavedMarker>()
    }

    pub(crate) fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        use std::hash::Hash;
        self.source_size
            .map(|(x, y)| (x.to_bits(), y.to_bits()))
            .hash(state);
        for line in &self.baselines {
            for value in [line.x, line.y, line.line_y, line.line_height] {
                value.to_bits().hash(state);
            }
            line.first.hash(state);
            line.end.hash(state);
        }
        for marker in &self.markers {
            for value in [marker.x, marker.y, marker.font_size] {
                value.to_bits().hash(state);
            }
        }
    }
}

impl SavedBaseline {
    pub(crate) fn from_value(value: &FigValue) -> Option<SavedBaseline> {
        let position = value.get("position")?;
        let (x, y) = (position.get_f64("x")?, position.get_f64("y")?);
        let line_y = value.get_f64("lineY")?;
        let line_height = value.get_f64("lineHeight")?;
        let first = value.get_f64("firstCharacter")?;
        let end = value.get_f64("endCharacter")?;
        if ![x, y, line_y, line_height, first, end]
            .into_iter()
            .all(f64::is_finite)
            || line_height <= 0.0
            || first < 0.0
            || end < first
            || first.fract() != 0.0
            || end.fract() != 0.0
        {
            return None;
        }
        Some(SavedBaseline {
            x,
            y,
            line_y,
            line_height,
            first: first as usize,
            end: end as usize,
        })
    }
}

impl SavedMarker {
    pub(crate) fn from_value(value: &FigValue) -> Option<Self> {
        let rotation = value.get_f64("rotation").unwrap_or(0.0);
        if value.get("firstCharacter").is_some() || !rotation.is_finite() || rotation.abs() > 0.001
        {
            return None;
        }
        let position = value.get("position")?;
        let (x, y, font_size) = (
            position.get_f64("x")?,
            position.get_f64("y")?,
            value.get_f64("fontSize")?,
        );
        (x.is_finite() && y.is_finite() && font_size.is_finite() && font_size > 0.0)
            .then_some(Self { x, y, font_size })
    }
}

pub(crate) fn has_unordered_list(node: &FigValue) -> bool {
    node.get("textData")
        .and_then(|text| text.get_array("lines"))
        .is_some_and(|lines| {
            lines
                .iter()
                .any(|line| line.get_str("lineType") == Some("UNORDERED_LIST"))
        })
}
