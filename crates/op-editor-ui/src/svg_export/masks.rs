//! Sibling mask runs, in the same topmost-first order as the canvas painter.

use super::*;
use crate::layout_scene::{MaskType, SceneFillLayer};
use jian_scene::path_geometry::flatten_path_points;

#[derive(Clone, Copy)]
pub(super) enum Operation {
    Clip,
    Alpha,
    Luminance,
    LuminanceLinear,
}

pub(super) fn marker(node: &SceneNode) -> Option<MaskType> {
    node.mask_type
        .or_else(|| node.is_mask.then_some(MaskType::Alpha))
}

pub(super) fn operation(node: &SceneNode) -> Option<Operation> {
    if node.hidden {
        return None;
    }
    let kind = marker(node)?;
    let clips = match kind {
        MaskType::Alpha => alpha_clip_is_exact(node),
        MaskType::Vector => vector_clip_is_exact(node),
        MaskType::Luminance | MaskType::LuminanceLinear => false,
    };
    if clips && has_clip_geometry(node) {
        return Some(Operation::Clip);
    }
    // Match the canvas's explicit fallback: general vector fill/stroke
    // coverage is not implemented there. Never substitute authored alpha.
    match kind {
        MaskType::Vector => None,
        MaskType::Alpha => Some(Operation::Alpha),
        MaskType::Luminance => Some(Operation::Luminance),
        MaskType::LuminanceLinear => Some(Operation::LuminanceLinear),
    }
}

pub(super) fn emit_siblings(out: &mut String, nodes: &[SceneNode]) {
    let mut active = false;
    for node in nodes.iter().rev() {
        if marker(node).is_some() {
            if active {
                out.push_str("</g>");
            }
            active = if let Some(operation) = operation(node) {
                emit_group_start(out, node, operation, false);
                true
            } else {
                false
            };
        } else {
            emit_node(out, node);
        }
    }
    if active {
        out.push_str("</g>");
    }
}

pub(super) fn resource_id(node: &SceneNode, op: Operation, ancestor: bool) -> String {
    let prefix = match (op, ancestor) {
        (Operation::Clip, false) => "vector-mask",
        (Operation::Clip, true) => "ancestor-vector-mask",
        (_, false) => "mask",
        (_, true) => "ancestor-mask",
    };
    format!("{prefix}-{}", svg_id(&node.id))
}

pub(super) fn emit_group_start(out: &mut String, node: &SceneNode, op: Operation, ancestor: bool) {
    let id = resource_id(node, op, ancestor);
    match op {
        Operation::Clip => {
            let _ = write!(out, r#"<g clip-path="url(#{id})">"#);
        }
        // Content first assembles in a transparent isolated layer, just as
        // canvas push_composite_layer followed by a DstIn mask source does.
        Operation::Alpha | Operation::Luminance | Operation::LuminanceLinear => {
            let _ = write!(out, r#"<g mask="url(#{id})" style="isolation:isolate">"#);
        }
    }
}

pub(super) fn emit_definition(
    out: &mut String,
    node: &SceneNode,
    parent_xform: glam::Affine2,
    ancestor: bool,
) {
    let Some(op) = operation(node) else { return };
    let id = resource_id(node, op, ancestor);
    if matches!(op, Operation::Clip) {
        let xform = parent_xform * node_transform(node);
        let _ = write!(
            out,
            r#"<clipPath id="{id}" clipPathUnits="userSpaceOnUse" transform="{}">"#,
            transform::matrix_value(xform)
        );
        emit_clip_geometry(out, node);
        out.push_str("</clipPath>");
        return;
    }
    let kind = match op {
        Operation::Luminance | Operation::LuminanceLinear => "luminance",
        _ => "alpha",
    };
    // Mask regions are derived from the complete source, not from a tile or
    // the content's untransformed bounding box. An empty source stays empty.
    let interpolation = if matches!(op, Operation::LuminanceLinear) {
        "linearRGB"
    } else {
        "sRGB"
    };
    let r = source_bounds(node, parent_xform).unwrap_or_else(|| Rect::xywh(0.0, 0.0, 1.0, 1.0));
    let _ = write!(
        out,
        r#"<mask id="{id}" maskUnits="userSpaceOnUse" maskContentUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}" mask-type="{kind}" style="mask-type:{kind}" color-interpolation="{interpolation}">"#,
        r.origin.x, r.origin.y, r.size.x, r.size.y
    );
    let transformed = parent_xform != glam::Affine2::IDENTITY;
    if transformed {
        emit_affine_group_start(out, parent_xform);
    }
    // Suppress only the mask root's node blend. Descendant blends, source
    // paint alpha, and isolated local opacity retain their usual semantics.
    render::emit_mask_source(out, node);
    if transformed {
        out.push_str("</g>");
    }
    out.push_str("</mask>");
}

pub(super) fn source_bounds(node: &SceneNode, parent_xform: glam::Affine2) -> Option<Rect> {
    let mut bounds = BoundsAcc::new();
    if matches!(operation(node), Some(Operation::Clip)) {
        let xform = parent_xform * node_transform(node);
        if node
            .svg_path
            .as_deref()
            .is_some_and(|d| !d.trim().is_empty())
        {
            return Some(transform::transformed_bounds(
                normalize_rect(node.bounds),
                xform,
            ));
        }
        let points = flatten_path_points(node);
        for point in points.as_slice() {
            let p = xform.transform_point2(*point);
            bounds.add(p.x, p.y, p.x, p.y);
        }
    } else {
        collect_bounds(node, parent_xform, &mut bounds);
    }
    bounds
        .into_rect()
        .filter(|r| r.size.x > 0.0 && r.size.y > 0.0)
}

fn has_clip_geometry(node: &SceneNode) -> bool {
    if node.bounds.size.x <= 0.0 || node.bounds.size.y <= 0.0 {
        return false;
    }
    node.svg_path
        .as_deref()
        .is_some_and(|d| !d.trim().is_empty())
        || (node.path_closed && flatten_path_points(node).as_slice().len() >= 3)
}

fn emit_clip_geometry(out: &mut String, node: &SceneNode) {
    if let Some(d) = node.svg_path.as_deref().filter(|d| !d.trim().is_empty()) {
        let rule = if node.even_odd_fill {
            "evenodd"
        } else {
            "nonzero"
        };
        let _ = write!(
            out,
            r#"<path d="{}"{} clip-rule="{rule}"/>"#,
            xml_escape(d),
            svg_path_fit_transform(d, node.bounds)
        );
    } else {
        let points = flatten_path_points(node);
        out.push_str("<polygon points=\"");
        for point in points.as_slice() {
            let _ = write!(out, "{},{} ", point.x, point.y);
        }
        out.push_str("\"/>");
    }
}

fn alpha_clip_is_exact(mask: &SceneNode) -> bool {
    if mask.kind != NodeKind::Path
        || mask.opacity < 0.999
        || mask.composite_opacity < 0.999
        || mask.stroke.is_some()
        || !mask.effects.is_empty()
        || !mask.children.is_empty()
    {
        return false;
    }
    if mask.fill_layers.is_empty() {
        return mask.fill.is_some_and(|color| color.a >= 0.999);
    }
    mask.fill_layers
        .iter()
        .any(|layer| matches!(layer, SceneFillLayer::Solid { color, .. } if color.a >= 0.999))
}

fn vector_clip_is_exact(mask: &SceneNode) -> bool {
    mask.kind == NodeKind::Path
        && mask.stroke.is_none()
        && mask.children.is_empty()
        && (!mask.fill_layers.is_empty()
            || mask.fill.is_some()
            || mask.gradient.is_some()
            || mask.image_src.is_some())
}
