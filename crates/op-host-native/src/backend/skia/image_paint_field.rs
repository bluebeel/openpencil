//! Image paint coordinates remain tied to the original stroked shape.
use super::{
    contain_rect,
    image::{cover_rect, figma_image_local_matrix, image_adjustment_matrix},
    layer::to_skia_blend_mode,
    to_sk_rect, NativeBackend,
};
use op_editor_ui::{ImageAdjustments, ImageBlendMode, ImageDrawMode, Rect};
pub(super) fn draw(
    backend: &mut NativeBackend,
    canvas: &skia_safe::Canvas,
    coverage: Rect,
    image: &skia_safe::Image,
    mode: ImageDrawMode,
    adjustments: ImageAdjustments,
    opacity: f32,
    transform: Option<[f32; 6]>,
    blend: ImageBlendMode,
    original_size: Option<[f32; 2]>,
    tile_scale: f32,
) {
    let rect = backend.active_paint_rect(coverage);
    let [w, h] = original_size
        .filter(|s| s.iter().all(|n| n.is_finite() && *n > 0.0))
        .unwrap_or([image.width() as f32, image.height() as f32]);
    let local = if mode == ImageDrawMode::Tile {
        None
    } else {
        transform.and_then(|m| {
            figma_image_local_matrix(rect, image.width() as f32, image.height() as f32, m)
        })
    };
    let local = local.unwrap_or_else(|| {
        let dst = match mode {
            ImageDrawMode::Fit => contain_rect(rect, w, h),
            ImageDrawMode::Fill | ImageDrawMode::Crop => cover_rect(rect, w, h),
            ImageDrawMode::Stretch => rect,
            ImageDrawMode::Tile => {
                let scale = if tile_scale.is_finite() && tile_scale > 0.0 {
                    tile_scale
                } else {
                    1.0
                };
                let (tw, th) = (w * scale, h * scale);
                Rect::xywh(
                    rect.origin.x + (rect.size.x - tw) * 0.5,
                    rect.origin.y + (rect.size.y - th) * 0.5,
                    tw,
                    th,
                )
            }
        };
        [
            dst.size.x / image.width() as f32,
            0.0,
            dst.origin.x,
            0.0,
            dst.size.y / image.height() as f32,
            dst.origin.y,
        ]
    });
    let matrix = skia_safe::Matrix::new_all(
        local[0], local[1], local[2], local[3], local[4], local[5], 0.0, 0.0, 1.0,
    );
    let tile = match mode {
        ImageDrawMode::Tile => skia_safe::TileMode::Repeat,
        ImageDrawMode::Fit => skia_safe::TileMode::Decal,
        _ => skia_safe::TileMode::Clamp,
    };
    let source = super::image_precision::minification_source(image, canvas, local);
    let Some(shader) = source.to_shader(
        (tile, tile),
        skia_safe::SamplingOptions::new(
            skia_safe::FilterMode::Linear,
            skia_safe::MipmapMode::Linear,
        ),
        &matrix,
    ) else {
        return;
    };
    let mut paint = skia_safe::Paint::default();
    paint.set_shader(shader);
    paint.set_alpha_f(opacity.clamp(0.0, 1.0));
    jian_skia::raster_sampling::set_blend_mode(&mut paint, to_skia_blend_mode(blend));
    if let Some(m) = image_adjustment_matrix(adjustments) {
        paint.set_color_filter(skia_safe::color_filters::matrix_row_major(&m, None));
    }
    canvas.draw_rect(to_sk_rect(coverage), &paint);
}
