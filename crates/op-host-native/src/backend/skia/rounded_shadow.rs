//! Profile-aware rounded shadows. The Gaussian entry stays the legacy draw.

use super::{jian_color_to_color4f, sampled_clip, to_sk_rect, NativeBackend};
use op_editor_ui::{RoundRectShadowOptions, ShadowBlurKernel};
use skia_safe::{Canvas, Paint, RRect};

impl NativeBackend {
    pub fn fill_round_rect_shadow(
        &mut self,
        canvas: &Canvas,
        rect: op_editor_ui::Rect,
        radius: f32,
        shadow: &RoundRectShadowOptions,
    ) -> bool {
        let shape = shadow.silhouette(rect, radius);
        if shadow.kernel == ShadowBlurKernel::Gaussian || shadow.blur <= 0.0 {
            if !shadow.inner {
                if let Some((bounds, radius)) = shape {
                    self.fill_drop_shadow(canvas, bounds, radius, shadow.blur, shadow.color);
                }
            } else {
                gaussian_inner(canvas, rect, radius, shape, shadow);
            }
            return true;
        }
        if shape.is_none() && !shadow.inner {
            return true;
        }
        if !shadow.blur.is_finite() || !radius.is_finite() {
            jian_skia::raster_sampling::mark_unsupported("nonfinite triangular shadow");
            return false;
        }
        let (bounds, corner) = shape
            .map(|(bounds, corner)| (to_sk_rect(bounds), corner))
            .unwrap_or((skia_safe::Rect::new_empty(), 0.0));
        let corner = corner
            .max(0.0)
            .min(bounds.width().min(bounds.height()) * 0.5);
        let saved = canvas.save();
        let draw_bounds = if shadow.inner {
            let mask = RRect::new_rect_xy(to_sk_rect(rect), radius, radius);
            sampled_clip::clip_rrect(canvas, &mask);
            to_sk_rect(rect).with_outset((1.0, 1.0))
        } else {
            bounds.with_outset((shadow.blur.max(0.0), shadow.blur.max(0.0)))
        };
        let drawn = jian_skia::triangular_shadow::draw(
            canvas,
            &jian_skia::triangular_shadow::RoundedShadowDraw {
                bounds,
                radius: corner,
                blur: shadow.blur,
                color: jian_color_to_color4f(shadow.color),
                inner: shadow.inner,
                draw_bounds,
            },
        );
        canvas.restore_to_count(saved);
        if !drawn {
            jian_skia::raster_sampling::mark_unsupported(
                "triangular shadow shader or framebuffer blend unavailable",
            );
        }
        drawn
    }
}

fn gaussian_inner(
    canvas: &Canvas,
    rect: op_editor_ui::Rect,
    radius: f32,
    shape: Option<(op_editor_ui::Rect, f32)>,
    shadow: &RoundRectShadowOptions,
) {
    let mask = RRect::new_rect_xy(to_sk_rect(rect), radius, radius);
    let saved = canvas.save();
    sampled_clip::clip_rrect(canvas, &mask);
    canvas.save_layer(&skia_safe::canvas::SaveLayerRec::default().bounds(&mask.rect()));
    let mut fill = Paint::new(jian_color_to_color4f(shadow.color), None);
    super::raster_point::prepare_solid(&mut fill);
    canvas.draw_rect(mask.rect(), &fill);
    if let Some((bounds, corner)) = shape {
        let mut cut = Paint::default();
        jian_skia::raster_sampling::set_blend_mode(&mut cut, skia_safe::BlendMode::DstOut);
        if shadow.blur > 0.0 {
            cut.set_mask_filter(skia_safe::MaskFilter::blur(
                skia_safe::BlurStyle::Normal,
                shadow.blur * 0.5,
                jian_skia::raster_sampling::shadow_respects_ctm(),
            ));
        }
        jian_skia::raster_sampling::configure_geometry(&mut cut);
        canvas.draw_round_rect(to_sk_rect(bounds), corner, corner, &cut);
    }
    canvas.restore_to_count(saved);
}
