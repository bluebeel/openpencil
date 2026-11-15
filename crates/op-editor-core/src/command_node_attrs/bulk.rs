//! Fuse independent, nonstructural attribute commands inside atomic Batch.
//!
//! A target's commands retain input order. Different targets only write their
//! own scalar/style fields and cannot observe each other's mutations. The
//! caller owns the Batch rollback checkpoint; this module never handles trees,
//! instance materialization, history or document metadata independently.

use std::collections::{BTreeMap, HashMap};

use crate::{EditorCommand as C, EditorState, NodeId, PenNodeExt};
use jian_ops_schema::node::PenNode;
use serde_json::{Map, Value};

use super::borrowed;

pub(crate) enum Prepared {
    Direct,
    NumericPatch(Map<String, Value>),
}

/// Parse supported rich geometry exactly once while gathering a run. Unknown,
/// null, sizing-keyword and structural patches remain ordinary boundaries.
pub(crate) fn prepare(command: &C) -> Option<Prepared> {
    match command {
        command if video_command(command) => None,
        C::PatchNodeData { patch_json, .. } => {
            crate::node_json::parse_numeric_geometry_patch(patch_json).map(Prepared::NumericPatch)
        }
        _ => target(command).map(|_| Prepared::Direct),
    }
}

// Video's ordinary instance-override path has additional editable-node
// semantics. Keep it a boundary in general Batch fusion; only the lexical
// canonical-node metadata route (which refuses virtual anchors) prepares it.
fn video_command(command: &C) -> bool {
    matches!(
        command,
        C::SetImageVideoSrc { .. }
            | C::SetImageVideoPlayback { .. }
            | C::AddImageVideo { .. }
            | C::RemoveImageVideo { .. }
    )
}
pub(crate) fn prepare_metadata(command: &C) -> Option<Prepared> {
    if video_command(command) {
        Some(Prepared::Direct)
    } else {
        prepare(command)
    }
}

pub(crate) fn target(command: &C) -> Option<&NodeId> {
    match command {
        C::UpdateNode { node_id, .. }
        | C::PatchNodeData { node_id, .. }
        | C::SetNodeRotation { node_id, .. }
        | C::SetNodeText { node_id, .. }
        | C::SetNodeCornerRadius { node_id, .. }
        | C::SetNodeFontSize { node_id, .. }
        | C::SetNodeFontWeight { node_id, .. }
        | C::SetNodeStrokeHex { node_id, .. }
        | C::SetNodeStrokeWidth { node_id, .. }
        | C::SetNodeStrokeSideWidth { node_id, .. }
        | C::SetNodeFillHex { node_id, .. }
        | C::SetNodeName { node_id, .. }
        | C::SetImageVideoSrc { node_id, .. }
        | C::SetImageVideoPlayback { node_id, .. }
        | C::AddImageVideo { node_id, .. }
        | C::RemoveImageVideo { node_id, .. }
        | C::SetNodeFlag { node_id, .. }
        | C::SetNodeFlip { node_id, .. }
        | C::AddNodeEffect { node_id, .. }
        | C::RemoveNodeEffect { node_id, .. }
        | C::SetEffectParam { node_id, .. }
        | C::SetEffectColor { node_id, .. } => Some(node_id),
        C::SetNodeLayoutProp {
            node_id, property, ..
        } if crate::command_layout_prop::is_borrowed_property(property) => Some(node_id),
        _ => None,
    }
}

/// One mutable preorder traversal per addressed page. Commands on each target
/// retain input order; duplicate IDs retain first-preorder authority per page.
/// The returned failure is the earliest original index, preserving the legacy
/// accepted-prefix revision counter when the caller restores its checkpoint.
pub(crate) fn apply(
    state: &mut EditorState,
    commands: &[C],
    prepared: &[Prepared],
) -> Result<(), usize> {
    if commands.len() != prepared.len() {
        return Err(0);
    }
    type Targets<'a> = HashMap<&'a str, Vec<usize>>;
    let mut pages: BTreeMap<usize, Targets<'_>> = BTreeMap::new();
    let mut first_failure = commands.len();
    for (index, command) in commands.iter().enumerate() {
        let Some(id) = target(command) else {
            return Err(index);
        };
        let page_id = match command {
            C::UpdateNode { page_id, .. } | C::PatchNodeData { page_id, .. } => page_id.as_deref(),
            _ => None,
        };
        match crate::command_apply::helpers::command_page_index(state, page_id) {
            Some(page) => pages
                .entry(page)
                .or_default()
                .entry(id.as_str())
                .or_default()
                .push(index),
            None => first_failure = first_failure.min(index),
        }
    }
    for (page, mut pending) in pages {
        if pending.values().all(|indices| indices[0] >= first_failure) {
            continue;
        }
        let roots = match state.doc.pages.as_mut() {
            Some(pages) if !pages.is_empty() => &mut pages[page].children,
            _ => &mut state.doc.children,
        };
        let mut stack = vec![roots.iter_mut()];
        while let Some(siblings) = stack.last_mut() {
            let Some(node) = siblings.next() else {
                stack.pop();
                continue;
            };
            if let Some(indices) = pending.remove(node.id_str()) {
                for index in indices {
                    if index >= first_failure {
                        break;
                    }
                    if !write(node, &commands[index], &prepared[index]) {
                        first_failure = index;
                        break;
                    }
                }
            }
            if pending.is_empty() {
                break;
            }
            // Searching must never turn an absent child vector into Some([]).
            if node.children().is_some() {
                if let Some(children) = node.children_mut() {
                    stack.push(children.iter_mut());
                }
            }
        }
        for indices in pending.values() {
            if let Some(index) = indices.first() {
                first_failure = first_failure.min(*index);
            }
        }
    }
    if first_failure == commands.len() {
        Ok(())
    } else {
        Err(first_failure)
    }
}

pub(crate) fn write(node: &mut PenNode, command: &C, prepared: &Prepared) -> bool {
    if target(command).is_none_or(|id| !id.is_real()) {
        return false;
    }
    if video_command(command)
        && (!node.base().visible.unwrap_or(true) || node.base().locked.unwrap_or(false))
    {
        return false;
    }
    match command {
        C::SetImageVideoSrc { src, .. } => crate::video_node_props::set_video_node_src(node, src),
        C::SetImageVideoPlayback { field, value, .. } => {
            crate::video_node_props::set_video_node_playback(node, *field, *value)
        }
        C::AddImageVideo { .. } => crate::video_node_props::add_video_to_image_node(node),
        C::RemoveImageVideo { .. } => crate::video_node_props::remove_video_from_image_node(node),
        C::UpdateNode {
            node_id,
            x,
            y,
            width,
            height,
            name,
            fill_hex,
            ..
        } => crate::command_node::borrowed_geometry::update_node(
            node, node_id, *x, *y, *width, *height, name, fill_hex,
        ),
        C::PatchNodeData { .. } => match prepared {
            Prepared::NumericPatch(patch) => {
                crate::node_json::apply_numeric_geometry_patch(node, patch)
            }
            Prepared::Direct => false,
        },
        C::SetNodeRotation { node_id, degrees } => {
            borrowed::set_node_rotation(node, node_id, *degrees)
        }
        C::SetNodeText { node_id, text } => borrowed::set_node_text(node, node_id, text),
        C::SetNodeCornerRadius { node_id, radius } => {
            borrowed::set_node_corner_radius(node, node_id, *radius)
        }
        C::SetNodeFontSize { node_id, font_size } => {
            borrowed::set_node_font_size(node, node_id, *font_size)
        }
        C::SetNodeFontWeight {
            node_id,
            font_weight,
        } => borrowed::set_node_font_weight(node, node_id, *font_weight),
        C::SetNodeStrokeHex { node_id, hex } => borrowed::set_node_stroke_hex(node, node_id, hex),
        C::SetNodeStrokeWidth { node_id, width } => {
            borrowed::set_node_stroke_width(node, node_id, *width)
        }
        C::SetNodeStrokeSideWidth {
            node_id,
            side,
            width,
        } => borrowed::set_node_stroke_side_width_exact(node, node_id, *side, f64::from(*width)),
        C::SetNodeFillHex { node_id, hex } => borrowed::set_node_fill_hex(node, node_id, hex),
        C::SetNodeName { node_id, name } => borrowed::set_node_name(node, node_id, name),
        C::SetNodeFlag {
            node_id,
            flag,
            value,
        } => borrowed::set_node_flag(node, node_id, *flag, *value),
        C::SetNodeFlip {
            node_id,
            flip_x,
            flip_y,
        } => borrowed::set_node_flip(node, node_id, *flip_x, *flip_y),
        C::AddNodeEffect { node_id, kind } => borrowed::add_node_effect(node, node_id, kind),
        C::RemoveNodeEffect { node_id, index } => {
            borrowed::remove_node_effect(node, node_id, *index)
        }
        C::SetEffectParam {
            node_id,
            index,
            field,
            value,
        } => borrowed::set_effect_param(node, node_id, *index, *field, *value),
        C::SetEffectColor {
            node_id,
            index,
            hex,
        } => borrowed::set_effect_color(node, node_id, *index, hex),
        C::SetNodeLayoutProp {
            property, value, ..
        } => crate::command_layout_prop::write_borrowed_property(node, property, value),
        _ => false,
    }
}
