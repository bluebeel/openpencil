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
    let mut pixels = vec![0_u8; 17 * 17 * 4];
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
                    assert!(
                        (i16::from(pixels[offset]) - 128).abs() <= 2,
                        "minification should integrate source detail: mode={mode:?}, affine={affine}, pixel={}",
                        pixels[offset]
                    );
                }
            }
        }
    }
}

#[test]
fn deep_minification_preserves_average_brightness_without_changing_cached_pixels() {
    let mut source = Vec::with_capacity(512 * 512 * 4);
    let mut sum = 0_u64;
    for index in 0_u32..512 * 512 {
        let value = (index.wrapping_mul(1_664_525).wrapping_add(1_013_904_223) >> 24) as u8;
        source.extend_from_slice(&[value, value, value, 255]);
        sum += u64::from(value);
    }
    let info = ImageInfo::new((512, 512), ColorType::RGBA8888, AlphaType::Premul, None);
    let image =
        skia_safe::images::raster_from_data(&info, skia_safe::Data::new_copy(&source), 512 * 4)
            .unwrap();
    let mut backend = NativeBackend::with_dpi(1.0);
    backend.install_raster_image(1, image, u32::MAX);
    let mut target = skia_safe::surfaces::raster_n32_premul((17, 17)).unwrap();
    backend.draw_image_with_mode(
        target.canvas(),
        Rect::xywh(0.0, 0.0, 17.0, 17.0),
        1,
        &[],
        ImageDrawMode::Fit,
    );
    let info = ImageInfo::new((17, 17), ColorType::RGBA8888, AlphaType::Unpremul, None);
    let mut pixels = vec![0_u8; 17 * 17 * 4];
    assert!(target.image_snapshot().read_pixels(
        &info,
        &mut pixels,
        17 * 4,
        (0, 0),
        skia_safe::image::CachingHint::Allow
    ));
    let actual = pixels.chunks_exact(4).map(|p| f64::from(p[0])).sum::<f64>() / (17.0 * 17.0);
    let expected = sum as f64 / (512.0 * 512.0);
    assert!(
        (actual - expected).abs() < 0.6,
        "mip levels must retain fractional averages: expected={expected}, actual={actual}"
    );
    assert_eq!(
        backend.raster_image(1).unwrap().color_type(),
        ColorType::RGBA8888
    );
}
