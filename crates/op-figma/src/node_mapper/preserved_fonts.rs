//! Preserve a coherent saved instance font when an unversioned override is stale.

use super::{effective_line_height_px, nearly_equal};
use crate::kiwi::FigValue;
use crate::text_cache::evidence::TextEvidence;
use crate::tree::guid_to_string;
use std::collections::HashMap;

struct Master {
    font: FigValue,
    size: f64,
    line_height: Option<f64>,
    characters: String,
}

fn path(entry: &FigValue) -> Option<Vec<String>> {
    entry
        .get("guidPath")?
        .get_array("guids")?
        .iter()
        .map(guid_to_string)
        .collect()
}

fn same_font(left: &FigValue, right: &FigValue) -> bool {
    ["family","style"].into_iter().all(|key|matches!((left.get_str(key),right.get_str(key)),(Some(a),Some(b)) if a.eq_ignore_ascii_case(b)))
}

pub(super) fn restore_saved_instance_fonts(nodes: &mut [FigValue]) {
    let masters: HashMap<_, _> = nodes
        .iter()
        .filter_map(|node| {
            Some((
                node.get("guid").and_then(guid_to_string)?,
                Master {
                    font: node.get("fontName")?.clone(),
                    size: node.get_f64("fontSize")?,
                    line_height: effective_line_height_px(node),
                    characters: node.get("textData")?.get_str("characters")?.into(),
                },
            ))
        })
        .collect();
    for node in nodes {
        let Some(overrides) = node
            .get("symbolData")
            .and_then(|data| data.get_array("symbolOverrides"))
        else {
            continue;
        };
        if !overrides.iter().any(|entry| {
            let Some(font) = entry.get("fontName") else {
                return false;
            };
            if entry.get_str("fontVersion").is_some_and(|v| !v.is_empty())
                || entry.get_bool("isOverrideOverTextStyle") == Some(true)
            {
                return false;
            }
            let Some(path) = path(entry) else {
                return false;
            };
            path.last()
                .and_then(|id| masters.get(id))
                .is_some_and(|master| !same_font(font, &master.font))
        }) {
            continue;
        }
        let Some(derived) = node.get_array("derivedSymbolData") else {
            continue;
        };
        let cached: HashMap<_, _> = derived
            .iter()
            .filter_map(|entry| Some((path(entry)?, entry)))
            .collect();
        let changes: Vec<_> = overrides
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let declared = entry.get("fontName")?;
                // A versioned or explicitly independent font is authored evidence;
                // this narrow repair is for unresolved/stale instance font names.
                if entry
                    .get_str("fontVersion")
                    .is_some_and(|version| !version.is_empty())
                    || entry.get_bool("isOverrideOverTextStyle") == Some(true)
                {
                    return None;
                }
                let key = path(entry)?;
                let master = masters.get(key.last()?)?;
                if same_font(declared, &master.font) {
                    return None;
                }
                let derived = cached.get(&key)?;
                let cache = derived.get("derivedTextData")?;
                let evidence = TextEvidence::from_cache(cache);
                if !evidence.uses_font(&master.font) {
                    return None;
                }
                let size = evidence.metric("glyphs", "fontSize")?;
                if !nearly_equal(size, master.size)
                    || !nearly_equal(size, entry.get_f64("fontSize").unwrap_or(master.size))
                {
                    return None;
                }
                let characters = entry
                    .get("textData")
                    .and_then(|data| data.get_str("characters"))
                    .unwrap_or(&master.characters);
                if characters.is_empty()
                    || evidence
                        .characters
                        .as_deref()
                        .is_some_and(|saved| saved != characters)
                {
                    return None;
                }
                let (width, height) = evidence.layout_size?;
                let placement = derived.get("size")?;
                if !nearly_equal(width, placement.get_f64("x")?)
                    || !nearly_equal(height, placement.get_f64("y")?)
                {
                    return None;
                }
                let layout = evidence.layout.as_ref()?;
                if evidence.saved_line_count()? != layout.baselines.len() {
                    return None;
                }
                let mut end = 0;
                for line in &layout.baselines {
                    if line.first != end
                        || line.end < line.first
                        || line.line_y < -0.001
                        || line.line_y + line.line_height > height + 0.001
                    {
                        return None;
                    }
                    end = line.end;
                }
                if end != characters.encode_utf16().count() {
                    return None;
                }
                let current_line_height = effective_line_height_px(entry).or(master.line_height)?;
                if !nearly_equal(
                    current_line_height,
                    evidence.metric("baselines", "lineHeight")?,
                ) {
                    return None;
                }
                Some((index, master.font.clone()))
            })
            .collect();
        if changes.is_empty() {
            continue;
        }
        let Some(overrides) = node
            .get_mut("symbolData")
            .and_then(|data| data.get_array_mut("symbolOverrides"))
        else {
            continue;
        };
        for (index, font) in changes {
            overrides[index].set("fontName", font);
        }
    }
}
