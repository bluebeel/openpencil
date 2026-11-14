//! Shared rounded-shadow dispatch and inset ordering for frame-like nodes.

use crate::layout_scene::{DropShadow, Effect, NodeKind, SceneNode};
use crate::widgets::canvas_viewport_fill_layers::paint_fill_layers;
use crate::widgets::canvas_viewport_image::paint_image_node_without_stroke;
use crate::widgets::canvas_viewport_overlay::paint_node_fill;
use crate::widgets::PaintCx;
use crate::{Point2D, Rect, RoundRectShadowOptions};

pub(super) fn paint_round_shadow(
    cx: &mut PaintCx<'_>,
    shadow: &DropShadow,
    rect: Rect,
    radius: f32,
    zoom: f32,
) {
    let options = RoundRectShadowOptions {
        offset: cx.backend.shadow_offset_in_local_space(Point2D::new(
            shadow.offset_x * zoom,
            shadow.offset_y * zoom,
        )),
        blur: shadow.blur * zoom,
        spread: shadow.spread * zoom,
        color: shadow.color,
        kernel: shadow.blur_kernel,
        inner: shadow.inner,
    };
    if !cx.backend.fill_round_rect_shadow(rect, radius, &options) && !shadow.inner {
        if let Some((bounds, radius)) = options.silhouette(rect, radius) {
            cx.backend
                .fill_drop_shadow(bounds, radius, options.blur, options.color);
        }
    }
}

pub(super) fn has_inset_shadow(node: &SceneNode) -> bool {
    matches!(node.kind, NodeKind::Frame | NodeKind::Rect)
        && node
            .corner_radii
            .is_none_or(|radii| radii.windows(2).all(|pair| pair[0] == pair[1]))
        && node
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::DropShadow(shadow) if shadow.inner))
}

/// Paint fill then insets; the caller paints its outline once after children.
pub(super) fn paint_fill_and_insets(
    cx: &mut PaintCx<'_>,
    node: &SceneNode,
    rect: Rect,
    zoom: f32,
    allow_overlays: bool,
) {
    if paint_fill_layers(cx, node, rect, zoom) {
        // All authored layers painted without an outline.
    } else if let Some(src) = node.image_src.as_deref() {
        paint_image_node_without_stroke(cx, node, rect, zoom, src, allow_overlays);
    } else {
        paint_node_fill(cx, node, rect, zoom, node.fill);
    }
    for effect in &node.effects {
        if let Effect::DropShadow(shadow) = effect {
            if shadow.inner {
                paint_round_shadow(
                    cx,
                    shadow,
                    rect,
                    node.corner_radii.map_or(node.corner_radius, |r| r[0]) * zoom,
                    zoom,
                );
            }
        }
    }
}
