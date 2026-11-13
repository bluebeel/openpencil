//! Source-aware stratified nearest sampling for imported image paints.

mod shader;
#[cfg(test)]
mod tests;

use super::{
    contain_rect,
    image::{cover_rect, figma_image_local_matrix, image_adjustment_matrix},
    layer::to_skia_blend_mode,
    sampled_clip, to_sk_rect, NativeBackend,
};
use op_editor_ui::{ImageDrawMode, ImagePaintOptions, Rect};
use skia_safe::{Canvas, Image, Matrix, RRect};

fn mapping(rect: Rect, image: &Image, options: &ImagePaintOptions<'_>) -> Option<Matrix> {
    if let Some(transform) = options.transform {
        let m =
            figma_image_local_matrix(rect, image.width() as f32, image.height() as f32, transform)?;
        return Some(Matrix::new_all(
            m[0], m[1], m[2], m[3], m[4], m[5], 0.0, 0.0, 1.0,
        ));
    }
    let [w, h] = options
        .original_size
        .filter(|v| v.iter().all(|n| n.is_finite() && *n > 0.0))
        .unwrap_or([image.width() as f32, image.height() as f32]);
    let destination = match options.mode {
        ImageDrawMode::Fit => contain_rect(rect, w, h),
        ImageDrawMode::Fill | ImageDrawMode::Crop => cover_rect(rect, w, h),
        ImageDrawMode::Stretch => rect,
        ImageDrawMode::Tile => return None,
    };
    Some(Matrix::new_all(
        destination.size.x / image.width() as f32,
        0.0,
        destination.origin.x,
        0.0,
        destination.size.y / image.height() as f32,
        destination.origin.y,
        0.0,
        0.0,
        1.0,
    ))
}

fn adequate(canvas: &Canvas, local: &Matrix, grid: f32) -> bool {
    let device = canvas.local_to_device_as_3x3();
    if device.has_perspective() || !device.is_finite() {
        return false;
    }
    let x = device.map_vector((local.scale_x(), local.skew_y()));
    let y = device.map_vector((local.skew_x(), local.scale_y()));
    let trace = f64::from(x.x).powi(2)
        + f64::from(x.y).powi(2)
        + f64::from(y.x).powi(2)
        + f64::from(y.y).powi(2);
    let det = f64::from(x.x) * f64::from(y.y) - f64::from(x.y) * f64::from(y.x);
    let maximum = (trace + (trace * trace - 4.0 * det * det).max(0.0).sqrt()) * 0.5;
    maximum.is_finite() && maximum * f64::from(grid).powi(2) <= 1.0 + 1e-6
}

pub(super) fn draw(
    backend: &mut NativeBackend,
    canvas: &Canvas,
    rect: Rect,
    id: u64,
    options: &ImagePaintOptions<'_>,
) -> bool {
    // Repeating image coordinates retain the existing Tile path until its
    // wrap-address sampling contract is supported by this point shader.
    if options.mode == ImageDrawMode::Tile {
        return false;
    }
    let grid = if sampled_clip::is_four_sample_pass() {
        1.0
    } else {
        2.0
    };
    let thumbnail = options
        .thumbnail
        .as_ref()
        .and_then(|thumb| backend.raster_image(thumb.id))
        .and_then(|image| {
            let local = mapping(rect, &image, options)?;
            adequate(canvas, &local, grid).then_some((image, local))
        });
    let selected = thumbnail.or_else(|| {
        let image = backend.raster_image(id)?;
        let local = mapping(rect, &image, options)?;
        Some((image, local))
    });
    let Some((image, local)) = selected else {
        return false;
    };
    let shape = RRect::new_rect_xy(
        to_sk_rect(rect),
        options.corner_radius.max(0.0),
        options.corner_radius.max(0.0),
    );
    let Some(shader) = shader::make(canvas, &image, &local, &shape, grid) else {
        return false;
    };
    let device = canvas.local_to_device_as_3x3();
    let Some(inverse) = device.invert() else {
        return false;
    };
    let (mut bounds, _) = device.map_rect(to_sk_rect(rect));
    bounds.outset((1.0, 1.0));
    let (draw_bounds, _) = inverse.map_rect(bounds);
    let mut paint = skia_safe::Paint::default();
    paint.set_shader(shader);
    paint.set_alpha_f(options.opacity.clamp(0.0, 1.0));
    paint.set_blend_mode(to_skia_blend_mode(options.blend_mode));
    if let Some(matrix) = image_adjustment_matrix(options.adjustments) {
        paint.set_color_filter(skia_safe::color_filters::matrix_row_major(&matrix, None));
    }
    canvas.draw_rect(draw_bounds, &paint);
    super::image_diagnostics::record_sharp_raster_hit();
    true
}
