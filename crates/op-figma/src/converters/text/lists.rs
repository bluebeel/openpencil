//! Preserve authored list line breaks and marker positions as editable text.
//!
//! This deliberately bakes saved geometry in Preserve mode. It does not add a
//! canonical paragraph/list model or promise reflow when those lines are edited.

use crate::common::{common_props, ConversionContext, FigLayoutMode};
use crate::kiwi::FigValue;
use crate::mappers::{map_figma_effects, map_figma_fills};
use crate::node_build::{group_node, text_node};
use crate::text_cache::layout::{SavedTextLayout, PRESERVED_TEXT_LAYOUT};
use crate::text_mapper::map_figma_text_props;
use jian_ops_schema::node::{
    ContainerProps, PenNode, PenNodeBase, TextAlign, TextAlignVertical, TextContent, TextGrowth,
};
use jian_ops_schema::sizing::SizingBehavior;

pub(super) fn convert_preserved_list(
    figma: &FigValue,
    id: &str,
    width: &SizingBehavior,
    height: &SizingBehavior,
    ctx: &mut ConversionContext<'_>,
) -> Option<PenNode> {
    if ctx.layout_mode != FigLayoutMode::Preserve {
        return None;
    }
    let FigValue::TextLayout(layout) = figma.get(PRESERVED_TEXT_LAYOUT)? else {
        return None;
    };
    let text = figma.get("textData")?;
    let characters = text.get_str("characters")?;
    let units: Vec<_> = characters.encode_utf16().collect();
    let paragraphs = paragraph_starts(text, &units)?;
    let (source_width, source_height) = layout.source_size?;
    let size = figma.get("size")?;
    if (source_width - size.get_f64("x")?).abs() > 0.001
        || (source_height - size.get_f64("y")?).abs() > 0.001
        || !valid_lines(layout, units.len())
    {
        return None;
    }
    // Validate every slice and marker before minting any child ids. A stale
    // cache falls back without changing the rest of the document's ids.
    let planned: Option<Vec<_>> = layout
        .baselines
        .iter()
        .map(|line| {
            let content = String::from_utf16(&units[line.first..line.end]).ok()?;
            let marker = if paragraphs
                .iter()
                .any(|(start, unordered)| *start == line.first && *unordered)
            {
                let mut matches = layout
                    .markers
                    .iter()
                    .filter(|marker| (marker.y - line.y).abs() <= 0.01 && marker.x < line.x);
                let marker = matches.next()?;
                if matches.next().is_some() {
                    return None;
                }
                Some(marker)
            } else {
                None
            };
            Some((line, content, marker))
        })
        .collect();
    let fills = map_figma_fills(figma.get_array("fillPaints"));
    let mut children = Vec::new();
    for (line, content, marker) in planned? {
        let mut sliced = figma.clone();
        let mut data = text.clone();
        data.set("characters", FigValue::Str(content.into()));
        if let Some(ids) = text.get_array("characterStyleIDs") {
            data.set(
                "characterStyleIDs",
                FigValue::Array(
                    ids[line.first.min(ids.len())..line.end.min(ids.len())]
                        .to_vec()
                        .into(),
                ),
            );
        }
        sliced.set("textData", data);
        let mut props = map_figma_text_props(&sliced);
        props.text_growth = Some(TextGrowth::Auto);
        props.text_align = Some(TextAlign::Left);
        props.text_align_vertical = Some(TextAlignVertical::Top);
        children.push(text_node(
            PenNodeBase {
                id: ctx.generate_id(),
                x: Some(line.x),
                y: Some(line.line_y),
                ..Default::default()
            },
            SizingBehavior::Number((source_width - line.x).max(0.0)),
            SizingBehavior::Number(line.line_height),
            props,
            fills.clone(),
            None,
        ));
        if let Some(marker) = marker {
            let mut props = map_figma_text_props(figma);
            props.content = TextContent::Plain("•".into());
            props.font_size = Some(marker.font_size);
            props.text_growth = Some(TextGrowth::Auto);
            props.text_align = Some(TextAlign::Left);
            props.text_align_vertical = Some(TextAlignVertical::Top);
            props.underline = None;
            props.strikethrough = None;
            children.push(text_node(
                PenNodeBase {
                    id: ctx.generate_id(),
                    name: Some("Bullet".into()),
                    x: Some(marker.x),
                    y: Some(line.line_y),
                    ..Default::default()
                },
                SizingBehavior::Number((line.x - marker.x).max(0.0)),
                SizingBehavior::Number(line.line_height),
                props,
                fills.clone(),
                None,
            ));
        }
    }
    Some(group_node(
        common_props(figma, id.into()),
        ContainerProps {
            width: Some(width.clone()),
            height: Some(height.clone()),
            effects: map_figma_effects(figma.get_array("effects")),
            ..Default::default()
        },
        Some(children),
    ))
}

fn paragraph_starts(text: &FigValue, units: &[u16]) -> Option<Vec<(usize, bool)>> {
    let metadata = text.get_array("lines")?;
    let starts: Vec<_> = std::iter::once(0)
        .chain(
            units
                .iter()
                .enumerate()
                .filter_map(|(index, unit)| (*unit == 10).then_some(index + 1)),
        )
        .collect();
    if metadata.len() != starts.len() {
        return None;
    }
    let mut found_list = false;
    let rows: Option<Vec<_>> = starts
        .into_iter()
        .zip(metadata)
        .map(|(start, line)| {
            let list = match line.get_str("lineType")? {
                "PLAIN" => false,
                "UNORDERED_LIST" => true,
                _ => return None,
            };
            found_list |= list;
            Some((start, list))
        })
        .collect();
    found_list.then_some(rows).flatten()
}

fn valid_lines(layout: &SavedTextLayout, len: usize) -> bool {
    let mut end = 0;
    let mut previous_y = f64::NEG_INFINITY;
    for line in &layout.baselines {
        if line.first != end || line.end > len || line.end < line.first || line.line_y <= previous_y
        {
            return false;
        }
        end = line.end;
        previous_y = line.line_y;
    }
    !layout.baselines.is_empty() && end == len
}

#[cfg(test)]
mod tests;
