//! Resolve full canonical stroke paints without enlarging the legacy Copy
//! descriptor or allocating extra data for default solid outlines.

use super::*;
use crate::payload::StrokePayload;
use jian_scene::{StrokePaintCap, StrokePaintJoin, StrokePaintStyle};
use std::sync::Arc;

pub(super) fn stroke_paint_to_scene(
    stroke: &StrokePayload,
    node_id: &op_editor_core::NodeId,
    variables: &VariableTable,
    size: [f32; 2],
) -> Option<Arc<jian_scene::SceneStrokePaint>> {
    let body = stroke.detail.as_deref()?;
    if body.fill.as_deref().is_some_and(|fills| {
        !fills
            .iter()
            .any(jian_ops_schema::style::PenFill::is_enabled)
    }) {
        return None;
    }
    let size = size.map(|v| if v == 0.0 { 1.0 } else { v });
    let variable_color = body
        .fill
        .as_deref()
        .and_then(|fills| fills.first())
        .is_none_or(jian_ops_schema::style::PenFill::is_enabled)
        .then(|| variables.stroke_color_for(node_id))
        .flatten();
    let mut layers = fill_layers_to_scene(
        body.fill.as_deref().unwrap_or_default(),
        variable_color,
        size,
    );
    if layers.is_empty() {
        layers.push(SceneFillLayer::Solid {
            color: variable_color
                .unwrap_or_else(|| array_to_color(stroke.color.unwrap_or([0.0, 0.0, 0.0, 1.0]))),
            blend_mode: ImageBlendMode::Normal,
        });
    }
    let cap = match body.cap.as_ref() {
        Some(jian_ops_schema::style::StrokeCap::Round) => StrokePaintCap::Round,
        Some(jian_ops_schema::style::StrokeCap::Square) => StrokePaintCap::Square,
        _ => StrokePaintCap::Butt,
    };
    let join = match body.join.as_ref() {
        Some(jian_ops_schema::style::StrokeJoin::Round) => StrokePaintJoin::Round,
        Some(jian_ops_schema::style::StrokeJoin::Bevel) => StrokePaintJoin::Bevel,
        _ => StrokePaintJoin::Miter,
    };
    let mut dash_pattern: Vec<f32> = body
        .dash_pattern
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|value| *value as f32)
        .collect();
    if dash_pattern
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
        || dash_pattern.iter().all(|value| *value == 0.0)
    {
        dash_pattern.clear();
    }
    if dash_pattern.len() % 2 != 0 {
        dash_pattern.extend_from_within(..);
    }
    let dash_offset = body.dash_offset.unwrap_or(0.0) as f32;
    Some(Arc::new(jian_scene::SceneStrokePaint {
        layers,
        style: StrokePaintStyle {
            cap,
            join,
            dash_pattern,
            dash_offset: if dash_offset.is_finite() {
                dash_offset
            } else {
                0.0
            },
        },
    }))
}
