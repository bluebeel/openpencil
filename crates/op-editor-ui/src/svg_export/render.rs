//! Node layers preserve the canvas composite and front-sibling mask order.

use super::*;
use crate::ImageBlendMode;

pub(super) fn emit_node(out: &mut String, n: &SceneNode) {
    emit_node_inner(out, n, false);
}

pub(super) fn emit_mask_source(out: &mut String, n: &SceneNode) {
    emit_node_inner(out, n, true);
}

fn emit_node_inner(out: &mut String, n: &SceneNode, suppress_node_blend: bool) {
    if n.hidden {
        return;
    }
    let local_xform = node_transform(n);
    let needs_g = local_xform != glam::Affine2::IDENTITY;
    if needs_g {
        emit_affine_group_start(out, local_xform);
    }
    let blend = if suppress_node_blend {
        ImageBlendMode::Normal
    } else {
        n.blend_mode
    };
    let needs_composite = emit_composite_start(out, n.composite_opacity, blend);
    let defers_rect_stroke = matches!(n.kind, NodeKind::Rect | NodeKind::Frame)
        && !n.children.is_empty()
        && n.stroke.is_some();
    if matches!(n.kind, NodeKind::Frame) && composite::has_widget_visual(n) {
        // Frame widgets retain their authored container behind the tab bar.
        emit_widget_frame_background(out, n, defers_rect_stroke);
    }
    let composite_painted = composite::emit_visual(out, n);
    if !composite_painted {
        match &n.kind {
            NodeKind::Rect | NodeKind::Frame if defers_rect_stroke => emit_rect_fill(out, n),
            NodeKind::Rect | NodeKind::Frame => emit_rect(out, n),
            NodeKind::Ellipse => emit_ellipse(out, n),
            NodeKind::Polygon => emit_polygon(out, n),
            NodeKind::Line => emit_line(out, n),
            NodeKind::Path => emit_path(out, n),
            NodeKind::Text => emit_text(out, n),
            NodeKind::Group => {}
            NodeKind::Other(_) => {
                if n.image_src.is_some() {
                    emit_image(out, n);
                } else if n.fill.is_some() || n.gradient.is_some() || n.stroke.is_some() {
                    emit_rect(out, n);
                }
            }
        }
    }
    let clips_children = n.clip_content
        && !n.children.is_empty()
        && n.bounds.size.x != 0.0
        && n.bounds.size.y != 0.0;
    if clips_children {
        let _ = write!(out, r#"<g clip-path="url(#clip-{})">"#, svg_id(&n.id));
    }
    masks::emit_siblings(out, n.visible_children());
    if clips_children {
        out.push_str("</g>");
    }
    if defers_rect_stroke && (!composite_painted || matches!(n.kind, NodeKind::Frame)) {
        emit_rect_stroke_overlay(out, n);
    }
    if needs_composite {
        out.push_str("</g>");
    }
    if needs_g {
        out.push_str("</g>");
    }
}

/// Node opacity and blend are applied once to the assembled subtree. Normal,
/// fully opaque nodes stay pass-through so child blends can see the backdrop.
pub(super) fn emit_composite_start(out: &mut String, opacity: f32, mode: ImageBlendMode) -> bool {
    let opacity = if opacity.is_finite() {
        opacity.clamp(0.0, 1.0)
    } else {
        1.0
    };
    if mode == ImageBlendMode::Normal {
        if opacity >= 1.0 {
            return false;
        }
        let _ = write!(out, r#"<g opacity="{opacity}">"#);
    } else {
        let name = match mode {
            ImageBlendMode::Normal => unreachable!(),
            ImageBlendMode::Darken => "darken",
            ImageBlendMode::Multiply => "multiply",
            ImageBlendMode::Screen => "screen",
            ImageBlendMode::Overlay => "overlay",
            ImageBlendMode::Lighten => "lighten",
            ImageBlendMode::Difference => "difference",
            ImageBlendMode::Hue => "hue",
            ImageBlendMode::Saturation => "saturation",
            ImageBlendMode::Color => "color",
            ImageBlendMode::Luminosity => "luminosity",
            ImageBlendMode::SoftLight => "soft-light",
            ImageBlendMode::ColorDodge => "color-dodge",
            ImageBlendMode::ColorBurn => "color-burn",
            ImageBlendMode::HardLight => "hard-light",
            ImageBlendMode::Exclusion => "exclusion",
        };
        let _ = write!(
            out,
            r#"<g opacity="{opacity}" style="mix-blend-mode:{name};isolation:isolate">"#
        );
    }
    true
}
