//! Minified images must average source detail instead of aliasing it.

use super::*;
use op_editor_ui::{ImageAdjustments, ImageBlendMode, ImageDrawMode};
use skia_safe::{AlphaType, ColorType, ImageInfo};

fn stripes() -> skia_safe::Image {
    let mut surface = skia_safe::surfaces::raster_n32_premul((512, 512)).unwrap();
    surface.canvas().clear(skia_safe::Color::BLACK);
    let paint = skia_safe::Paint::new(skia_safe::Color4f::new(1.0, 1.0, 1.0, 1.0), None);
    for x in (1..512).step_by(2) {
        surface.canvas().draw_rect(
            skia_safe::Rect::from_xywh(x as f32, 0.0, 1.0, 512.0),
            &paint,
        );
    }
    surface.image_snapshot()
}

fn minified(mode: ImageDrawMode, affine: bool) -> Vec<u8> {
    let mut backend = NativeBackend::with_dpi(1.0);
    backend.install_raster_image(1, stripes(), u32::MAX);
    let mut target = skia_safe::surfaces::raster_n32_premul((17, 17)).unwrap();
    target.canvas().clear(skia_safe::Color::TRANSPARENT);
    backend.draw_image_with_options_transform_and_blend(
        target.canvas(),
        Rect::xywh(0.0, 0.0, 17.0, 17.0),
        1,
        &[],
        mode,
        ImageAdjustments::default(),
        1.0,
        0.0,
        affine.then_some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
        ImageBlendMode::Normal,
    );
    let info = ImageInfo::new((17, 17), ColorType::RGBA8888, AlphaType::Unpremul, None);
    let mut pixels = vec![0; 17 * 17 * 4];
    assert!(target.image_snapshot().read_pixels(
        &info,
        &mut pixels,
        17 * 4,
        (0, 0),
        skia_safe::image::CachingHint::Allow
    ));
    pixels
}

#[test]
fn minification_filters_source_detail_for_affine_and_standard_image_modes() {
    // The source alternates black/white every pixel. At this nonintegral
    // reduction ratio each output pixel covers about 30 stripes and should
    // converge to mid-gray; nearest and bilinear-only sampling visibly alias.
    for mode in [
        ImageDrawMode::Fit,
        ImageDrawMode::Stretch,
        ImageDrawMode::Fill,
        ImageDrawMode::Crop,
    ] {
        for affine in [false, true] {
            let pixels = minified(mode, affine);
            for y in 2..15 {
                for x in 2..15 {
                    let offset = (y * 17 + x) * 4;
                    assert_eq!(pixels[offset + 3], 255);
                    assert!((i16::from(pixels[offset]) - 128).abs() <= 2,
                        "minification should integrate source detail: mode={mode:?}, affine={affine}, pixel={}", pixels[offset]);
                }
            }
        }
    }
}
