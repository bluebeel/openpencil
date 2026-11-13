//! Device-space multisample coverage for rounded clips.
//!
//! Four rotated-grid samples at 2x resolution resolve to sixteen samples in
//! an output pixel. Keep sample offsets in device space under zoom and skew.

use skia_safe::{runtime_effect::RuntimeShaderBuilder, Canvas, Matrix, RRect, RuntimeEffect};
use std::cell::Cell;

const SOURCE: &str = r#"
uniform float4 bounds;
uniform float4 topRadii;
uniform float4 bottomRadii;
uniform float4 deviceInverse;
uniform float grid;

float ellipse(float2 p, float2 center, float2 radius) {
    float2 q = (p - center) / radius;
    return dot(q, q) <= 1.0 ? 1.0 : 0.0;
}
float inside(float2 p) {
    if (p.x < bounds.x || p.y < bounds.y || p.x >= bounds.z || p.y >= bounds.w) return 0.0;
    if (p.x < bounds.x + topRadii.x && p.y < bounds.y + topRadii.y)
        return ellipse(p, bounds.xy + topRadii.xy, topRadii.xy);
    if (p.x > bounds.z - topRadii.z && p.y < bounds.y + topRadii.w)
        return ellipse(p, float2(bounds.z - topRadii.z, bounds.y + topRadii.w), topRadii.zw);
    if (p.x > bounds.z - bottomRadii.x && p.y > bounds.w - bottomRadii.y)
        return ellipse(p, bounds.zw - bottomRadii.xy, bottomRadii.xy);
    if (p.x < bounds.x + bottomRadii.z && p.y > bounds.w - bottomRadii.w)
        return ellipse(p, float2(bounds.x + bottomRadii.z, bounds.w - bottomRadii.w), bottomRadii.zw);
    return 1.0;
}
float sampleAt(float2 p, float2 offset) {
    return inside(p + float2(deviceInverse.x * offset.x + deviceInverse.y * offset.y,
                             deviceInverse.z * offset.x + deviceInverse.w * offset.y));
}
half4 main(float2 p) {
    if (grid == 0.0) return half4(inside(p));
    float coverage = 0.0;
    for (int y = 0; y < 2; ++y) {
        for (int x = 0; x < 2; ++x) {
            if (float(x) < grid && float(y) < grid) {
                float2 tile = float2(float(x), float(y));
                coverage += sampleAt(p, (tile + float2(.375, .125)) / grid - .5);
                coverage += sampleAt(p, (tile + float2(.875, .375)) / grid - .5);
                coverage += sampleAt(p, (tile + float2(.125, .625)) / grid - .5);
                coverage += sampleAt(p, (tile + float2(.625, .875)) / grid - .5);
            }
        }
    }
    return half4(coverage / (4.0 * grid * grid));
}
"#;

thread_local! {
    static EFFECT: Option<RuntimeEffect> = RuntimeEffect::make_for_shader(SOURCE, None).ok();
    static GRID: Cell<f32> = const { Cell::new(2.0) };
}

pub(super) fn is_four_sample_pass() -> bool {
    GRID.with(|grid| grid.get() == 1.0)
}

pub(super) fn with_four_samples<R>(draw: impl FnOnce() -> R) -> R {
    struct Restore(f32);
    impl Drop for Restore {
        fn drop(&mut self) {
            GRID.with(|grid| grid.set(self.0));
        }
    }
    let restore = Restore(GRID.with(|grid| grid.replace(1.0)));
    let result = draw();
    drop(restore);
    result
}

pub(super) fn clip_rrect(canvas: &Canvas, rrect: &RRect) {
    let matrix = canvas.local_to_device_as_3x3();
    let shader = (!matrix.has_perspective())
        .then(|| matrix.invert())
        .flatten()
        .and_then(|inverse| {
            EFFECT.with(|effect| {
                let mut builder = RuntimeShaderBuilder::new(effect.as_ref()?.clone());
                let rect = rrect.rect();
                let radii = rrect.radii_ref();
                let values: [(&str, &[f32]); 5] = [
                    ("bounds", &[rect.left, rect.top, rect.right, rect.bottom]),
                    (
                        "topRadii",
                        &[radii[0].x, radii[0].y, radii[1].x, radii[1].y],
                    ),
                    (
                        "bottomRadii",
                        &[radii[2].x, radii[2].y, radii[3].x, radii[3].y],
                    ),
                    (
                        "deviceInverse",
                        &[
                            inverse.scale_x(),
                            inverse.skew_x(),
                            inverse.skew_y(),
                            inverse.scale_y(),
                        ],
                    ),
                    (
                        "grid",
                        &[if jian_skia::raster_sampling::is_point_sampling() {
                            0.0
                        } else {
                            GRID.with(Cell::get)
                        }],
                    ),
                ];
                for (name, value) in values {
                    builder.set_uniform_float(name, value).ok()?;
                }
                builder.make_shader(&Matrix::default())
            })
        });
    if let Some(shader) = shader {
        canvas.clip_shader(shader, skia_safe::ClipOp::Intersect);
    } else {
        jian_skia::raster_sampling::mark_unsupported("rounded clip shader unavailable");
        canvas.clip_rrect(
            rrect,
            skia_safe::ClipOp::Intersect,
            jian_skia::raster_sampling::geometry_antialias(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_coverage_uses_device_samples_under_zoom() {
        for zoom in [1.0, 2.0] {
            let mut surface = skia_safe::surfaces::raster_n32_premul((64, 64)).unwrap();
            let canvas = surface.canvas();
            canvas.clear(skia_safe::Color::TRANSPARENT);
            canvas.scale((zoom, zoom));
            let side = 64.0 / zoom;
            clip_rrect(
                canvas,
                &RRect::new_oval(skia_safe::Rect::from_wh(side, side)),
            );
            canvas.clear(skia_safe::Color::WHITE);
            let image = surface.image_snapshot();
            let info = skia_safe::ImageInfo::new(
                (64, 64),
                skia_safe::ColorType::RGBA8888,
                skia_safe::AlphaType::Unpremul,
                None,
            );
            let mut pixels = vec![0u8; 64 * 64 * 4];
            assert!(image.read_pixels(
                &info,
                &mut pixels,
                64 * 4,
                (0, 0),
                skia_safe::image::CachingHint::Allow
            ));
            assert_eq!(pixels[3], 0);
            assert_eq!(
                pixels[31 * 4 + 3],
                255,
                "all samples at top-center lie inside"
            );
            assert!(
                (i16::from(pixels[24 * 4 + 3]) - 32).abs() <= 1,
                "two of sixteen samples lie inside at this edge"
            );
            assert_eq!(pixels[(32 * 64 + 32) * 4 + 3], 255);
        }
    }
}
