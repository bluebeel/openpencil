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

struct ImageMapping {
    local: Matrix,
    rotation_uv: Option<[f32; 6]>,
}

fn mapping(rect: Rect, image: &Image, options: &ImagePaintOptions<'_>) -> Option<ImageMapping> {
    let rotation_uv = super::image_rotation::supported(
        rect,
        [image.width() as f32, image.height() as f32],
        options,
    );
    if let Some(transform) = options.transform.or(rotation_uv) {
        let m =
            figma_image_local_matrix(rect, image.width() as f32, image.height() as f32, transform)?;
        return Some(ImageMapping {
            local: Matrix::new_all(m[0], m[1], m[2], m[3], m[4], m[5], 0.0, 0.0, 1.0),
            rotation_uv,
        });
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
    Some(ImageMapping {
        local: Matrix::new_all(
            destination.size.x / image.width() as f32,
            0.0,
            destination.origin.x,
            0.0,
            destination.size.y / image.height() as f32,
            destination.origin.y,
            0.0,
            0.0,
            1.0,
        ),
        rotation_uv: None,
    })
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
        jian_skia::raster_sampling::mark_unsupported("nearest tiled image coordinates");
        return false;
    }
    let point = jian_skia::raster_sampling::is_point_sampling();
    let grid = if point || sampled_clip::is_four_sample_pass() {
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
            adequate(canvas, &local.local, grid).then_some((image, local))
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
    let Some(shader) = shader::make_with_rotation(
        canvas,
        &image,
        &local.local,
        local.rotation_uv,
        &shape,
        if point { 0.0 } else { grid },
    ) else {
        if point {
            jian_skia::raster_sampling::mark_unsupported("nearest point image shader");
        }
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
    // grid0 emits byte-normalized colors and binary geometry alpha. The
    // child uses NEAREST/CLAMP, so an opaque source cannot acquire Decal or
    // filter alpha. Final paint opacity/adjustment filters are checked too.
    if point && options.blend_mode == op_editor_ui::ImageBlendMode::Normal {
        jian_skia::raster_sampling::configure_final_byte_image(&mut paint, image.is_opaque());
    } else {
        jian_skia::raster_sampling::configure_paint(&mut paint);
    }
    canvas.draw_rect(draw_bounds, &paint);
    super::image_diagnostics::record_sharp_raster_hit();
    true
}
