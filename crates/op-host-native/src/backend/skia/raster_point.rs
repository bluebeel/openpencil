//! Preserve float paint inputs that the ordinary packed-color bridge narrows.

use super::{jian_color_to_color4f, to_sk_rect};
use op_editor_ui::{Color, Rect};
use skia_safe::{
    runtime_effect::ChildPtr, AlphaType, Canvas, ColorType, Data, ImageInfo, Matrix, Paint,
    RuntimeEffect, SamplingOptions,
};

#[path = "raster_round_rect.rs"]
mod rounded_rect;

pub(super) fn draw_round_rect(
    canvas: &Canvas,
    rect: skia_safe::Rect,
    radius_x: f32,
    radius_y: f32,
    paint: &Paint,
) {
    let rrect = skia_safe::RRect::new_rect_xy(rect, radius_x, radius_y);
    if !rounded_rect::draw(canvas, &rrect, paint) {
        canvas.draw_round_rect(rect, radius_x, radius_y, paint);
    }
}

pub(super) fn draw_rrect(canvas: &Canvas, rect: &skia_safe::RRect, paint: &Paint) {
    if !rounded_rect::draw(canvas, rect, paint) {
        canvas.draw_rrect(rect, paint);
    }
}

/// Point cameras must use one vector edge rule regardless of whether the
/// source was a glyph, editable path, polygon or a gradient silhouette. Normal
/// Native rendering and filtered/path-effect paints keep their existing draw.
pub(super) fn draw_path(canvas: &Canvas, path: &skia_safe::Path, paint: &Paint) {
    if jian_skia::raster_sampling::is_point_sampling()
        && paint.mask_filter().is_none()
        && paint.image_filter().is_none()
        && paint.path_effect().is_none()
        && super::vector_text::draw_path(canvas, path, paint)
    {
        return;
    }
    canvas.draw_path(path, paint);
}

pub(super) fn clip_path(canvas: &Canvas, path: &skia_safe::Path) {
    if jian_skia::raster_sampling::is_point_sampling()
        && super::vector_text::clip_path(canvas, path)
    {
        return;
    }
    canvas.clip_path(
        path,
        skia_safe::ClipOp::Intersect,
        jian_skia::raster_sampling::geometry_antialias(),
    );
}

pub(super) fn draw_line(
    canvas: &Canvas,
    from: op_editor_ui::Point2D,
    to: op_editor_ui::Point2D,
    paint: &Paint,
) {
    if jian_skia::raster_sampling::is_point_sampling() {
        let mut path = skia_safe::PathBuilder::new();
        path.move_to((from.x, from.y));
        path.line_to((to.x, to.y));
        draw_path(canvas, &path.detach(), paint);
    } else {
        canvas.draw_line((from.x, from.y), (to.x, to.y), paint);
    }
}

pub(super) fn draw_oval(canvas: &Canvas, bounds: skia_safe::Rect, paint: &Paint) {
    if jian_skia::raster_sampling::is_point_sampling() {
        draw_path(canvas, &skia_safe::Path::oval(bounds, None), paint);
    } else {
        canvas.draw_oval(bounds, paint);
    }
}

pub(super) fn rect(canvas: &Canvas, rect: Rect, color: Color, stroke: Option<f32>) -> bool {
    if !jian_skia::raster_sampling::is_point_sampling() {
        return false;
    }
    let mut paint = Paint::new(jian_color_to_color4f(color), None);
    if let Some(width) = stroke {
        paint.set_stroke(true);
        paint.set_stroke_width(width);
    }
    jian_skia::raster_sampling::configure_final_src_over(&mut paint);
    canvas.draw_rect(to_sk_rect(rect), &paint);
    true
}

/// Prepare an ordinary solid draw after its color and filters are final.
pub(super) fn prepare_solid(paint: &mut Paint) {
    jian_skia::raster_sampling::configure_geometry(paint);
    jian_skia::raster_sampling::configure_final_src_over(paint);
}

const MAX_VERTICES: usize = 65536;
const MESH_SOURCE: &str = r#"
uniform shader colors;
uniform float4 bounds;
uniform float2 extent;

half4 main(float2 p){
    float2 q=clamp((p-bounds.xy)/bounds.zw,0.0,1.0)*extent;
    int2 cell=int2(min(floor(q),extent-1.0));
    float2 f=q-float2(cell);
    float2 vertex=float2(cell)+.5;
    float4 tl=float4(colors.eval(vertex));
    float4 tr=float4(colors.eval(vertex+float2(1.0,0.0)));
    float4 bl=float4(colors.eval(vertex+float2(0.0,1.0)));
    float4 br=float4(colors.eval(vertex+float2(1.0,1.0)));
    float4 color=f.x+f.y<=1.0 ? tl*(1.0-f.x-f.y)+tr*f.x+bl*f.y
        : tr*(1.0-f.y)+br*(f.x+f.y-1.0)+bl*(1.0-f.x);
    return half4(color);
}
"#;
thread_local! {static MESH:Option<RuntimeEffect>=RuntimeEffect::make_for_shader(MESH_SOURCE,None).ok();}

#[allow(clippy::too_many_arguments)]
pub(super) fn mesh(
    canvas: &Canvas,
    rect: Rect,
    radius: f32,
    rows: u32,
    cols: u32,
    colors: &[Color],
    opacity: f32,
) -> bool {
    mesh_field(
        canvas, rect, rect, radius, rows, cols, colors, opacity, true,
    )
}
#[allow(clippy::too_many_arguments)]
pub(super) fn mesh_field(
    canvas: &Canvas,
    coverage: Rect,
    rect: Rect,
    radius: f32,
    rows: u32,
    cols: u32,
    colors: &[Color],
    opacity: f32,
    point_only: bool,
) -> bool {
    if point_only && !jian_skia::raster_sampling::is_point_sampling() {
        return false;
    }
    let (rows, cols) = (rows.max(2), cols.max(2));
    let count = rows.checked_mul(cols).map(|n| n as usize);
    if count.is_none_or(|n| n > MAX_VERTICES || n != colors.len()) {
        jian_skia::raster_sampling::mark_unsupported(
            "invalid float mesh or more than65536 vertices",
        );
        return true;
    }
    if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
        return true;
    }
    let shader = MESH.with(|effect| {
        let effect = effect.as_ref()?;
        let mut values = Vec::with_capacity(colors.len() * 16);
        for color in colors {
            let alpha = color.a.clamp(0.0, 1.0);
            for value in [
                color.r.clamp(0.0, 1.0) * alpha,
                color.g.clamp(0.0, 1.0) * alpha,
                color.b.clamp(0.0, 1.0) * alpha,
                alpha,
            ] {
                values.extend_from_slice(&value.to_ne_bytes());
            }
        }
        let info = ImageInfo::new(
            (cols as i32, rows as i32),
            ColorType::RGBAF32,
            AlphaType::Premul,
            None,
        );
        let vertices = skia_safe::images::raster_from_data(
            &info,
            Data::new_copy(&values),
            cols as usize * 16,
        )?;
        let child = vertices.to_shader(None, SamplingOptions::default(), None)?;
        let values = [
            rect.origin.x,
            rect.origin.y,
            rect.size.x,
            rect.size.y,
            (cols - 1) as f32,
            (rows - 1) as f32,
        ];
        let bytes: Vec<_> = values.into_iter().flat_map(f32::to_ne_bytes).collect();
        effect.make_shader(
            Data::new_copy(&bytes),
            &[ChildPtr::Shader(child)],
            Some(&Matrix::default()),
        )
    });
    let Some(shader) = shader else {
        jian_skia::raster_sampling::mark_unsupported("float mesh shader unavailable");
        return true;
    };
    let mut paint = Paint::default();
    paint.set_shader(shader);
    paint.set_alpha_f(opacity.clamp(0.0, 1.0));
    jian_skia::raster_sampling::configure_paint(&mut paint);
    draw_round_rect(canvas, to_sk_rect(coverage), radius, radius, &paint);
    true
}

#[cfg(all(test, not(target_os = "windows")))]
#[path = "raster_point_tests.rs"]
mod tests;
