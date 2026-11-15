use super::*;
use op_editor_ui::{ImageBlendMode, ImagePaintThumbnail};

fn solid(size: i32, color: skia_safe::Color) -> Image {
    let mut surface = skia_safe::surfaces::raster_n32_premul((size, size)).unwrap();
    surface.canvas().clear(color);
    surface.image_snapshot()
}

fn paint(
    backend: &mut NativeBackend,
    zoom: f32,
    nearest: bool,
    opacity: f32,
    blend: ImageBlendMode,
    background: skia_safe::Color,
) -> Image {
    let mut surface = skia_safe::surfaces::raster_n32_premul((8, 8)).unwrap();
    surface.canvas().clear(background);
    surface.canvas().scale((zoom, zoom));
    let options = ImagePaintOptions {
        nearest_sampling: nearest,
        thumbnail: Some(ImagePaintThumbnail {
            id: 2,
            encoded: &[],
        }),
        original_size: Some([64.0, 64.0]),
        opacity,
        blend_mode: blend,
        ..Default::default()
    };
    backend.draw_image_with_paint_options(
        surface.canvas(),
        Rect::xywh(0.0, 0.0, 2.0, 2.0),
        1,
        &[],
        &options,
    );
    surface.image_snapshot()
}

#[test]
fn resident_thumbnail_is_selected_only_when_it_covers_the_sampling_footprint() {
    let mut backend = NativeBackend::with_dpi(1.0);
    backend.install_raster_image(1, solid(64, skia_safe::Color::RED), u32::MAX);
    backend.install_raster_image(2, solid(8, skia_safe::Color::BLUE), u32::MAX);
    for (zoom, nearest, expected) in [
        (1.0, true, skia_safe::Color::BLUE),
        (3.0, true, skia_safe::Color::RED),
        (1.0, false, skia_safe::Color::RED),
    ] {
        let image = paint(
            &mut backend,
            zoom,
            nearest,
            1.0,
            ImageBlendMode::Normal,
            skia_safe::Color::TRANSPARENT,
        );
        assert_eq!(image.peek_pixels().unwrap().get_color((1, 1)), expected);
    }
}

#[test]
fn ready_thumbnail_renders_without_original_and_blend_opacity_apply_once() {
    let mut backend = NativeBackend::with_dpi(1.0);
    backend.install_raster_image(2, solid(8, skia_safe::Color::BLUE), u32::MAX);
    let image = paint(
        &mut backend,
        1.0,
        true,
        0.5,
        ImageBlendMode::Multiply,
        skia_safe::Color::YELLOW,
    );
    let pixels = image.peek_pixels().unwrap();
    let color = pixels.get_color((1, 1));
    assert!((127..=128).contains(&color.r()));
    assert!((127..=128).contains(&color.g()));
    assert_eq!(color.b(), 0);
    assert_eq!(color.a(), 255);
    assert_eq!(pixels.get_color((2, 1)), skia_safe::Color::YELLOW);
}
