//! Rich text shares the shape/stroke paint-field compositor. The glyph and
//! decoration coverage is painted once after assembling all authored layers.
use super::{decorate_slice, draw_slice, SliceStyle};
use crate::layout_scene::{SceneFillLayer, SceneNode};
use crate::widgets::{canvas_viewport_fill_layers::paint_clipped_shape_rich_fill_layer, PaintCx};
use crate::{
    Color, ImageBlendMode, Rect, TextDecorationRequest, TextMaskAction as A, TextMaskResult as R,
};

struct Restore<'a, 'b> {
    cx: &'a mut PaintCx<'b>,
    layers: usize,
    paint_space: bool,
    prepared: bool,
}
impl Restore<'_, '_> {
    fn restore(&mut self) {
        self.cx.backend.restore();
        self.layers -= 1;
    }
}
impl Drop for Restore<'_, '_> {
    fn drop(&mut self) {
        if self.prepared {
            self.cx.backend.text_mask_capture(A::Discard);
        }
        if self.paint_space {
            self.cx.backend.pop_paint_space();
        }
        while self.layers > 0 {
            self.restore();
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_slice(
    cx: &mut PaintCx<'_>,
    node: &SceneNode,
    paint_rect: Rect,
    text: &str,
    style: SliceStyle<'_>,
    family: &str,
    x: f32,
    baseline: f32,
    spacing: f32,
    justify: f32,
) -> f32 {
    paint_slice_inner(
        cx, node, paint_rect, text, style, family, x, baseline, spacing, justify, true,
    )
}
#[allow(clippy::too_many_arguments)]
fn paint_slice_inner(
    cx: &mut PaintCx<'_>,
    node: &SceneNode,
    paint_rect: Rect,
    text: &str,
    mut style: SliceStyle<'_>,
    family: &str,
    x: f32,
    baseline: f32,
    spacing: f32,
    justify: f32,
    prepare: bool,
) -> f32 {
    if let Some(layers) = style.paints.filter(|_| style.enabled) {
        // An explicitly empty stack never falls back to the legacy ink color.
        if layers.is_empty() {
            style.enabled = false;
        } else if let [SceneFillLayer::Solid {
            blend_mode: ImageBlendMode::Normal,
            ..
        }] = layers
        {
            // The existing resolved color already contains this solid's alpha,
            // node opacity and variable projection. Preserve it byte-for-byte.
        } else if cx.backend.supports_stroke_paint_fields() && cx.backend.supports_pixel_masks() {
            if let Some(full_area) = cx.backend.paint_field_bounds().filter(valid_area) {
                let original_style = style;
                let mut scope = Restore {
                    cx: &mut *cx,
                    layers: 0,
                    paint_space: false,
                    prepared: false,
                };
                let mut prepared = None;
                if prepare && matches!(scope.cx.backend.text_mask_capture(A::Begin), R::Recording) {
                    scope.prepared = true;
                    let mask = SliceStyle {
                        color: Color::WHITE,
                        paints: None,
                        ..style
                    };
                    let next = draw(
                        scope.cx, node, text, mask, family, x, baseline, spacing, justify,
                    );
                    if let R::Ready(bounds) = scope.cx.backend.text_mask_capture(A::Finish) {
                        prepared = Some((next, bounds));
                    } else {
                        scope.cx.backend.text_mask_capture(A::Discard);
                        scope.prepared = false;
                    }
                }
                let area = prepared
                    .as_ref()
                    .and_then(|(_, bounds)| *bounds)
                    .filter(valid_support)
                    .map(|bounds| intersect(bounds, full_area))
                    .unwrap_or(full_area);
                if area.size.x <= 0.0 || area.size.y <= 0.0 {
                    return prepared.map_or(x, |(next, _)| next);
                }
                scope
                    .cx
                    .backend
                    .push_composite_layer(area, node.opacity, ImageBlendMode::Normal);
                scope.layers += 1;
                scope.cx.backend.push_paint_space(paint_rect);
                scope.paint_space = true;
                // Canonical arrays are front-to-back. Layer alpha/blending
                // stays authored; node opacity applies to the assembled stack.
                // Only the layer allocation hint is tight. Keep each field's
                // original finite draw extent, avoiding new AA paint edges or
                // changing image/shader coordinates at the support boundary.
                for layer in layers.iter().rev() {
                    let isolated = layer.blend_mode() != ImageBlendMode::Normal;
                    if isolated {
                        scope
                            .cx
                            .backend
                            .push_composite_layer(area, 1.0, layer.blend_mode());
                        scope.layers += 1;
                    }
                    if let SceneFillLayer::Solid { color, .. } = layer {
                        scope.cx.backend.fill_rect(full_area, *color);
                    } else {
                        paint_clipped_shape_rich_fill_layer(scope.cx, node, layer, full_area, 1.0);
                    }
                    if isolated {
                        scope.restore();
                    }
                }
                scope.cx.backend.pop_paint_space();
                scope.paint_space = false;
                scope.cx.backend.push_mask_source_layer(false);
                scope.layers += 1;
                // Original text layout, glyph-local baseline rounding, cluster
                // tracking and metric decoration gaps all remain on draw_slice.
                style.color = Color::WHITE;
                style.paints = None;
                if let Some((next, _)) = prepared {
                    if matches!(scope.cx.backend.text_mask_capture(A::Replay), R::Replayed) {
                        scope.prepared = false;
                        return next;
                    }
                    // A refused replay is guaranteed to draw nothing. Its
                    // empty DstIn layer clears the pending outer paint stack;
                    // release both before the one ordinary full-support retry.
                    scope.cx.backend.text_mask_capture(A::Discard);
                    scope.prepared = false;
                    scope.restore();
                    scope.restore();
                    drop(scope);
                    return paint_slice_inner(
                        cx,
                        node,
                        paint_rect,
                        text,
                        original_style,
                        family,
                        x,
                        baseline,
                        spacing,
                        justify,
                        false,
                    );
                }
                return draw(
                    scope.cx, node, text, style, family, x, baseline, spacing, justify,
                );
            }
        }
    }
    // Public/custom painters that lack mask/field support keep their old solid
    // projection. Production Native and CanvasKit implement the rich path.
    draw(cx, node, text, style, family, x, baseline, spacing, justify)
}
fn intersect(a: Rect, b: Rect) -> Rect {
    let (x, y) = (a.origin.x.max(b.origin.x), a.origin.y.max(b.origin.y));
    let (r, d) = (
        (a.origin.x + a.size.x).min(b.origin.x + b.size.x),
        (a.origin.y + a.size.y).min(b.origin.y + b.size.y),
    );
    Rect::xywh(x, y, (r - x).max(0.0), (d - y).max(0.0))
}
fn valid_support(area: &Rect) -> bool {
    [
        area.origin.x,
        area.origin.y,
        area.size.x,
        area.size.y,
        area.origin.x + area.size.x,
        area.origin.y + area.size.y,
    ]
    .into_iter()
    .all(f32::is_finite)
        && area.size.x >= 0.0
        && area.size.y >= 0.0
}
fn valid_area(area: &Rect) -> bool {
    [area.origin.x, area.origin.y, area.size.x, area.size.y]
        .into_iter()
        .all(f32::is_finite)
        && area.size.x > 0.0
        && area.size.y > 0.0
}
#[allow(clippy::too_many_arguments)]
fn draw(
    cx: &mut PaintCx<'_>,
    node: &SceneNode,
    text: &str,
    style: SliceStyle<'_>,
    family: &str,
    x: f32,
    baseline: f32,
    spacing: f32,
    justify: f32,
) -> f32 {
    let (next, end) = draw_slice(
        cx.backend,
        text,
        style,
        family,
        x,
        baseline,
        spacing,
        justify,
        node.font_kerning,
        &node.font_features,
    );
    let handled = style.enabled
        && style.text_decoration_skip_ink.is_some_and(|skip_ink| {
            (style.underline || style.strikethrough)
                && cx.backend.draw_text_decoration(&TextDecorationRequest {
                    text,
                    font_family: family,
                    font_size: style.font_size,
                    font_weight: style.weight,
                    italic: style.italic,
                    kerning: node.font_kerning,
                    letter_spacing: spacing,
                    line_height: node.line_height,
                    justify_extra: justify,
                    font_features: &node.font_features,
                    origin: crate::Point2D::new(x, baseline),
                    width: end - x,
                    color: style.color,
                    underline: style.underline,
                    strikethrough: style.strikethrough,
                    skip_ink,
                })
        });
    if !handled {
        decorate_slice(cx.backend, style, x, end, baseline);
    }
    next
}
