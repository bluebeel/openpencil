//! Per-corner outer/inset profiles; uniform inputs retain the original path.
use super::{jian_color_to_color4f, raster_point, sampled_clip, to_sk_rect, NativeBackend};
use op_editor_ui::{Rect, RoundRectShadowOptions, ShadowBlurKernel};
use skia_safe::{Canvas, Paint, Point, RRect};

fn rrect(rect: Rect, radii: [f32; 4]) -> RRect {
    RRect::new_rect_radii(
        to_sk_rect(rect),
        &radii.map(|r| Point::new(r.max(0.0), r.max(0.0))),
    )
}
impl NativeBackend {
    pub fn fill_round_rect_shadow_per_corner(
        &mut self,
        canvas: &Canvas,
        rect: Rect,
        radii: [f32; 4],
        shadow: &RoundRectShadowOptions,
    ) -> bool {
        if radii.windows(2).all(|p| p[0] == p[1]) {
            return self.fill_round_rect_shadow(canvas, rect, radii[0], shadow);
        }
        if !radii.iter().all(|r| r.is_finite()) || !shadow.blur.is_finite() {
            return false;
        }
        let shape = shadow.silhouette_per_corner(rect, radii);
        if shape.is_none() && !shadow.inner {
            return true;
        }
        if shadow.kernel == ShadowBlurKernel::Triangular && shadow.blur > 0.0 {
            let saved = canvas.save();
            if shadow.inner {
                sampled_clip::clip_rrect(canvas, &rrect(rect, radii));
            }
            let (bounds, corners) = shape
                .map(|(r, c)| (to_sk_rect(r), c))
                .unwrap_or((skia_safe::Rect::new_empty(), [0.0; 4]));
            let drawn = jian_skia::triangular_corner_shadow::draw(
                canvas,
                &jian_skia::triangular_corner_shadow::CornerShadowDraw {
                    bounds,
                    radii: corners,
                    blur: shadow.blur,
                    color: jian_color_to_color4f(shadow.color),
                    inner: shadow.inner,
                    draw_bounds: if shadow.inner {
                        to_sk_rect(rect).with_outset((1.0, 1.0))
                    } else {
                        bounds.with_outset((shadow.blur, shadow.blur))
                    },
                },
            );
            canvas.restore_to_count(saved);
            if !drawn {
                jian_skia::raster_sampling::mark_unsupported(
                    "per-corner triangular shadow shader unavailable",
                );
            }
            return drawn;
        }
        let mut fill = Paint::new(jian_color_to_color4f(shadow.color), None);
        if !shadow.inner {
            if let Some((bounds, corners)) = shape {
                if shadow.blur > 0.0 {
                    fill.set_mask_filter(skia_safe::MaskFilter::blur(
                        skia_safe::BlurStyle::Normal,
                        shadow.blur * 0.5,
                        jian_skia::raster_sampling::shadow_respects_ctm(),
                    ));
                }
                raster_point::prepare_solid(&mut fill);
                canvas.draw_rrect(rrect(bounds, corners), &fill);
            }
            return true;
        }
        let saved = canvas.save();
        let mask = rrect(rect, radii);
        sampled_clip::clip_rrect(canvas, &mask);
        canvas.save_layer(&skia_safe::canvas::SaveLayerRec::default().bounds(&mask.rect()));
        raster_point::prepare_solid(&mut fill);
        canvas.draw_rect(mask.rect(), &fill);
        if let Some((bounds, corners)) = shape {
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
            canvas.draw_rrect(rrect(bounds, corners), &cut);
        }
        canvas.restore_to_count(saved);
        true
    }
}
