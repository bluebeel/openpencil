//! Resolve image shading and four-sample coverage from a 2x device grid.

use skia_safe::{
    BlendMode, Canvas, FilterMode, ImageInfo, MipmapMode, Paint, Rect, SamplingOptions,
};

const MAX_PHYSICAL_EDGE: i32 = 8192;
const MAX_PHYSICAL_PIXELS: i64 = 16 * 1024 * 1024;

fn physical_size(bounds: Rect) -> Option<(i32, i32)> {
    let width = bounds.width() as i64 * 2;
    let height = bounds.height() as i64 * 2;
    (width > 0
        && height > 0
        && width <= i64::from(MAX_PHYSICAL_EDGE)
        && height <= i64::from(MAX_PHYSICAL_EDGE)
        && width * height <= MAX_PHYSICAL_PIXELS)
        .then_some((width as i32, height as i32))
}

/// Return true when the image was handled (including an empty visible area).
/// Unsupported transforms and allocation limits fall back to the caller's
/// ordinary image path. The persistent source-image cache is unchanged.
pub(super) fn draw(
    canvas: &Canvas,
    rect: Rect,
    opacity: f32,
    blend: BlendMode,
    paint_image: impl FnOnce(&Canvas),
) -> bool {
    if super::sampled_clip::is_four_sample_pass() {
        return false;
    }
    let matrix = canvas.local_to_device_as_3x3();
    if !matrix.is_finite() || matrix.has_perspective() || matrix.invert().is_none() {
        return false;
    }
    let (mut bounds, _) = matrix.map_rect(rect);
    if !bounds.is_finite() {
        return false;
    }
    let Some(clip) = canvas.device_clip_bounds() else {
        return true;
    };
    if !bounds.intersect(Rect::from_irect(clip)) {
        return true;
    }
    bounds = Rect::new(
        bounds.left.floor(),
        bounds.top.floor(),
        bounds.right.ceil(),
        bounds.bottom.ceil(),
    );
    let Some(size) = physical_size(bounds) else {
        return false;
    };
    let info = ImageInfo::new_n32_premul(size, canvas.image_info().color_space());
    let Some(mut surface) = skia_safe::surfaces::raster(&info, None, None) else {
        return false;
    };
    let inner = surface.canvas();
    inner.clear(skia_safe::Color::TRANSPARENT);
    inner.scale((2.0, 2.0));
    inner.translate((-bounds.left, -bounds.top));
    inner.concat(&matrix);
    // The inner image uses Normal at full opacity. Its per-quadrant color
    // and coverage resolve together; backdrop blend and opacity apply once.
    super::sampled_clip::with_four_samples(|| paint_image(inner));
    let mut paint = Paint::default();
    paint.set_blend_mode(blend);
    paint.set_alpha_f(opacity.clamp(0.0, 1.0));
    let saved = canvas.save();
    canvas.reset_matrix();
    canvas.draw_image_rect_with_sampling_options(
        surface.image_snapshot(),
        None,
        bounds,
        SamplingOptions::new(FilterMode::Linear, MipmapMode::None),
        &paint,
    );
    canvas.restore_to_count(saved);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_grid_preserves_transform_and_applies_opacity_once() {
        let mut surface = skia_safe::surfaces::raster_n32_premul((32, 32)).unwrap();
        let canvas = surface.canvas();
        canvas.clear(skia_safe::Color::WHITE);
        canvas.translate((10.0, 8.0));
        canvas.scale((2.0, 2.0));
        let rect = Rect::from_xywh(1.0, 2.0, 3.0, 4.0);
        assert!(draw(canvas, rect, 0.5, BlendMode::SrcOver, |inner| {
            assert!(super::super::sampled_clip::is_four_sample_pass());
            inner.draw_rect(
                rect,
                &Paint::new(skia_safe::Color4f::new(0.0, 0.0, 1.0, 1.0), None),
            );
        }));
        assert!(!super::super::sampled_clip::is_four_sample_pass());
        let image = surface.image_snapshot();
        let pixels = image.peek_pixels().unwrap();
        let inside = pixels.get_color((14, 14));
        assert!((127..=128).contains(&inside.r()));
        assert!((127..=128).contains(&inside.g()));
        assert_eq!(inside.b(), 255);
        assert_eq!(pixels.get_color((11, 14)), skia_safe::Color::WHITE);
        assert_eq!(pixels.get_color((18, 14)), skia_safe::Color::WHITE);
    }

    #[test]
    fn bounded_targets_and_nested_passes_fall_back_without_painting() {
        assert_eq!(
            physical_size(Rect::from_xywh(0.0, 0.0, 4096.0, 1024.0)),
            Some((8192, 2048))
        );
        assert_eq!(
            physical_size(Rect::from_xywh(0.0, 0.0, 4096.0, 1025.0)),
            None
        );
        assert_eq!(physical_size(Rect::from_xywh(0.0, 0.0, 4097.0, 1.0)), None);
        let mut surface = skia_safe::surfaces::raster_n32_premul((1, 1)).unwrap();
        let canvas = surface.canvas();
        canvas.scale((0.001, 0.001));
        super::super::sampled_clip::with_four_samples(|| {
            assert!(!draw(
                canvas,
                Rect::from_xywh(0.0, 0.0, 1.0, 1.0),
                1.0,
                BlendMode::SrcOver,
                |_| panic!("nested pass")
            ));
        });
    }
}
