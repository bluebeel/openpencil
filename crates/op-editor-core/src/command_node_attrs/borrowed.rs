//! Exact borrowed-node writers shared by individual commands and bulk runs.
//! Only target lookup moved out; validation, variants and authored values are
//! the same operations used by the ordinary command path.

use super::slots::{
    node_effects_slot, node_stroke_slot, set_stroke_side_width, set_stroke_width_preserving_sides,
    sided_stroke_thickness, stroke_side_index, stroke_side_widths, write_corner_radius,
};
use crate::command::{EffectField, NodeFlag, StrokeSide};
use crate::fills::{set_primary_fill_hex, set_primary_stroke_hex};
use crate::{NodeId, PenNodeExt};
use jian_ops_schema::node::{FontWeight, PenNode, TextContent};
use jian_ops_schema::style::{BlurBody, PenEffect, PenStroke, ShadowBody, StrokeThickness};

pub(crate) fn set_node_rotation(node: &mut PenNode, node_id: &NodeId, degrees: f32) -> bool {
    if !node_id.is_real() || !degrees.is_finite() {
        return false;
    }
    node.base_mut().rotation = Some(degrees as f64);
    true
}

pub(crate) fn set_node_text(node: &mut PenNode, node_id: &NodeId, text: &str) -> bool {
    if !node_id.is_real() {
        return false;
    }
    match node {
        PenNode::Text(t) => {
            t.content = TextContent::Plain(text.to_string());
            true
        }
        _ => false,
    }
}

pub(crate) fn set_node_corner_radius(node: &mut PenNode, node_id: &NodeId, radius: f32) -> bool {
    if !node_id.is_real() || !radius.is_finite() || radius < 0.0 {
        return false;
    }
    write_corner_radius(node, radius as f64)
}

pub(crate) fn set_node_font_size(node: &mut PenNode, node_id: &NodeId, font_size: f32) -> bool {
    if !node_id.is_real() || !font_size.is_finite() || font_size <= 0.0 {
        return false;
    }
    match node {
        PenNode::Text(t) => {
            t.font_size = Some((font_size as f64).into());
            true
        }
        _ => false,
    }
}

pub(crate) fn set_node_font_weight(node: &mut PenNode, node_id: &NodeId, font_weight: u16) -> bool {
    if !node_id.is_real() || font_weight == 0 || font_weight > 1000 {
        return false;
    }
    match node {
        PenNode::Text(t) => {
            t.font_weight = Some(FontWeight::Number(font_weight as u32));
            true
        }
        _ => false,
    }
}

pub(crate) fn set_node_stroke_hex(node: &mut PenNode, node_id: &NodeId, hex: &str) -> bool {
    if !node_id.is_real() {
        return false;
    }
    let is_var_ref = hex.starts_with('$');
    if !is_var_ref && crate::color_picker::parse_hex_rgb(hex).is_none() {
        return false;
    }
    set_primary_stroke_hex(node, hex)
}

pub(crate) fn set_node_stroke_width(node: &mut PenNode, node_id: &NodeId, width: f32) -> bool {
    if !node_id.is_real() || !width.is_finite() || width < 0.0 {
        return false;
    }
    let Some(slot) = node_stroke_slot(node) else {
        return false;
    };
    if width == 0.0 {
        *slot = None;
    } else {
        match slot {
            Some(stroke) => set_stroke_width_preserving_sides(stroke, width),
            none @ None => {
                *none = Some(PenStroke::from(jian_ops_schema::style::PenStrokeBody {
                    thickness: StrokeThickness::Uniform(f64::from(width)),
                    align: None,
                    join: None,
                    cap: None,
                    dash_pattern: None,
                    dash_offset: None,
                    fill: None,
                }));
            }
        }
    }
    true
}

pub(crate) fn set_node_stroke_side_width_exact(
    node: &mut PenNode,
    node_id: &NodeId,
    side: StrokeSide,
    width: f64,
) -> bool {
    if !node_id.is_real() || !width.is_finite() || width < 0.0 {
        return false;
    }
    let Some(slot) = node_stroke_slot(node) else {
        return false;
    };
    let mut widths = match slot.as_ref() {
        Some(stroke) => stroke_side_widths(&stroke.thickness),
        None => [0.0; 4],
    };
    widths[stroke_side_index(side)] = width;
    if widths.iter().all(|value| *value <= 0.0) {
        *slot = None;
        return true;
    }
    match slot {
        Some(stroke) => set_stroke_side_width(stroke, side, width),
        none @ None => {
            *none = Some(PenStroke::from(jian_ops_schema::style::PenStrokeBody {
                thickness: sided_stroke_thickness(widths),
                align: None,
                join: None,
                cap: None,
                dash_pattern: None,
                dash_offset: None,
                fill: None,
            }));
        }
    }
    true
}

pub(crate) fn set_node_fill_hex(node: &mut PenNode, node_id: &NodeId, hex: &str) -> bool {
    if !node_id.is_real() {
        return false;
    }
    let is_var_ref = hex.starts_with('$');
    if !is_var_ref && crate::color_picker::parse_hex_rgb(hex).is_none() {
        return false;
    }
    set_primary_fill_hex(node, hex)
}

pub(crate) fn set_node_name(node: &mut PenNode, node_id: &NodeId, name: &str) -> bool {
    if !node_id.is_real() {
        return false;
    }
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return false;
    }
    node.base_mut().name = Some(trimmed.to_string());
    true
}

pub(crate) fn set_node_flag(
    node: &mut PenNode,
    node_id: &NodeId,
    flag: NodeFlag,
    value: bool,
) -> bool {
    if !node_id.is_real() {
        return false;
    }
    if matches!(flag, NodeFlag::Collapsed) {
        // No `collapsed` field on `PenNodeBase` — collapse is a
        // layer-panel UI flag, not part of the `.op` document.
        return false;
    }
    match flag {
        // `visible == false` is the hidden state, so a `Hidden`
        // write inverts the sense.
        NodeFlag::Hidden => node.base_mut().visible = Some(!value),
        NodeFlag::Locked => node.base_mut().locked = Some(value),
        NodeFlag::Collapsed => unreachable!("rejected above"),
    }
    true
}

pub(crate) fn set_node_flip(
    node: &mut PenNode,
    node_id: &NodeId,
    flip_x: Option<bool>,
    flip_y: Option<bool>,
) -> bool {
    if !node_id.is_real() || (flip_x.is_none() && flip_y.is_none()) {
        return false;
    }
    if let Some(fx) = flip_x {
        node.base_mut().flip_x = Some(fx);
    }
    if let Some(fy) = flip_y {
        node.base_mut().flip_y = Some(fy);
    }
    true
}

pub(crate) fn add_node_effect(node: &mut PenNode, node_id: &NodeId, kind: &str) -> bool {
    if !node_id.is_real() {
        return false;
    }
    let effect = match kind {
        "shadow" => PenEffect::Shadow(ShadowBody {
            blur_kernel: None,
            inner: None,
            visible: None,
            offset_x: 4.0,
            offset_y: 4.0,
            blur: 8.0,
            spread: 0.0,
            color: "#00000040".to_string(),
        }),
        "blur" => PenEffect::Blur(BlurBody {
            radius: 4.0,
            visible: None,
        }),
        "background_blur" => PenEffect::BackgroundBlur(BlurBody {
            radius: 10.0,
            visible: None,
        }),
        _ => return false,
    };
    let Some(slot) = node_effects_slot(node) else {
        return false;
    };
    slot.get_or_insert_with(Vec::new).push(effect);
    true
}

pub(crate) fn remove_node_effect(node: &mut PenNode, node_id: &NodeId, index: u32) -> bool {
    if !node_id.is_real() {
        return false;
    }
    let Some(slot) = node_effects_slot(node) else {
        return false;
    };
    let Some(effects) = slot.as_mut() else {
        return false;
    };
    let i = index as usize;
    if i >= effects.len() {
        return false;
    }
    effects.remove(i);
    if effects.is_empty() {
        *slot = None;
    }
    true
}

pub(crate) fn set_effect_param(
    node: &mut PenNode,
    node_id: &NodeId,
    index: u32,
    field: EffectField,
    value: f32,
) -> bool {
    if !node_id.is_real() || !value.is_finite() {
        return false;
    }
    let Some(slot) = node_effects_slot(node) else {
        return false;
    };
    let Some(effects) = slot.as_mut() else {
        return false;
    };
    let Some(effect) = effects.get_mut(index as usize) else {
        return false;
    };
    let value = f64::from(value);
    match (effect, field) {
        (PenEffect::Shadow(s), EffectField::OffsetX) => s.offset_x = value,
        (PenEffect::Shadow(s), EffectField::OffsetY) => s.offset_y = value,
        (PenEffect::Shadow(s), EffectField::Blur) => s.blur = value.max(0.0),
        (PenEffect::Shadow(s), EffectField::Spread) => s.spread = value,
        (PenEffect::Blur(b), EffectField::Radius)
        | (PenEffect::BackgroundBlur(b), EffectField::Radius) => b.radius = value.max(0.0),
        _ => return false,
    }
    true
}

pub(crate) fn set_effect_color(
    node: &mut PenNode,
    node_id: &NodeId,
    index: u32,
    hex: &str,
) -> bool {
    if !node_id.is_real() {
        return false;
    }
    let Some(slot) = node_effects_slot(node) else {
        return false;
    };
    let Some(effects) = slot.as_mut() else {
        return false;
    };
    let Some(effect) = effects.get_mut(index as usize) else {
        return false;
    };
    match effect {
        PenEffect::Shadow(s) => {
            s.color = hex.to_string();
            true
        }
        _ => false,
    }
}
