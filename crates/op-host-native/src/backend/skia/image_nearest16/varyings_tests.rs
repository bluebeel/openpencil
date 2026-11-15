use super::*;
use skia_safe::{AlphaType, BlendMode, ColorType, Data, ImageInfo, Paint, RRect};

const IDENTITY_UV: [f32; 6] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

#[test]
fn varying_cameras_reject_nonfinite_singular_perspective_and_unsafe_ranges() {
    let rect = Rect::from_wh(64.0, 64.0);
    for matrix in [
        Matrix::scale((0.0, 1.0)),
        Matrix::translate((f32::INFINITY, 0.0)),
        Matrix::new_all(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.01, 0.0, 1.0),
        Matrix::translate((1.0e10, 0.0)),
        Matrix::scale((1.0e-8, 1.0e8)),
        Matrix::scale((100.0, 100.0)),
    ] {
        assert!(uniforms(&matrix, rect, IDENTITY_UV, 2.0).is_none());
    }
    assert!(uniforms(&Matrix::default(), rect, IDENTITY_UV, 3.0).is_none());
    assert!(uniforms(&Matrix::default(), Rect::new_empty(), IDENTITY_UV, 2.0).is_none());
    let mut invalid = IDENTITY_UV;
    invalid[0] = f32::NAN;
    assert!(uniforms(&Matrix::default(), rect, invalid, 2.0).is_none());
}

#[test]
fn varying_cameras_support_all_sample_grids_reflection_and_noncommutative_transforms() {
    let rect = Rect::from_xywh(7.0, 11.0, 64.0, 43.0);
    for matrix in [
        Matrix::translate((12.125, 9.375)),
        Matrix::new_all(1.25, 0.375, 17.0, -0.25, 0.75, 23.0, 0.0, 0.0, 1.0),
        Matrix::new_all(-1.5, 0.25, 174.0, 0.5, 0.875, 19.0, 0.0, 0.0, 1.0),
        Matrix::new_all(0.0, -2.0, 140.0, 2.0, 0.0, 3.0, 0.0, 0.0, 1.0),
    ] {
        for grid in [0.0, 1.0, 2.0] {
            let values = uniforms(&matrix, rect, IDENTITY_UV, grid).unwrap();
            assert!(values.into_iter().all(f32::is_finite));
        }
    }
}

fn point_texels(device: Matrix, uv: [f32; 6], rotated: bool) {
    let bytes: Vec<u8> = (0..16)
        .flat_map(|i| [(i % 4 * 50) as u8, (i / 4 * 50) as u8, 17, 255])
        .collect();
    let info = ImageInfo::new((4, 4), ColorType::RGBA8888, AlphaType::Unpremul, None);
    let image = skia_safe::images::raster_from_data(&info, Data::new_copy(&bytes), 16).unwrap();
    let rect = Rect::from_xywh(7.0, 11.0, 4.0, 4.0);
    let inverse_uv = Matrix::new_all(uv[0], uv[1], uv[2], uv[3], uv[4], uv[5], 0.0, 0.0, 1.0)
        .invert()
        .unwrap();
    let mut local = inverse_uv;
    local.pre_scale((1.0 / 4.0, 1.0 / 4.0), None);
    local.post_scale((4.0, 4.0), None);
    local.post_translate((rect.left, rect.top));
    let mut surface = skia_safe::surfaces::raster_n32_premul((96, 96)).unwrap();
    let canvas = surface.canvas();
    canvas.concat(&device);
    let shape = RRect::new_rect_xy(rect, 0.0, 0.0);
    let shader = super::super::shader::make_with_vertex_uv(
        canvas,
        &image,
        &local,
        rotated.then_some(uv),
        Some(uv),
        &shape,
        0.0,
    )
    .unwrap();
    let mut paint = Paint::default();
    paint.set_shader(shader);
    paint.set_blend_mode(BlendMode::Src);
    canvas.draw_rect(rect, &paint);
    let snapshot = surface.image_snapshot();
    let pixels = snapshot.peek_pixels().unwrap();
    for y in 0..4 {
        for x in 0..4 {
            let device_point =
                device.map_point((rect.left + x as f32 + 0.5, rect.top + y as f32 + 0.5));
            let color = pixels.get_color((device_point.x as i32, device_point.y as i32));
            let (sx, sy) = if rotated {
                (y, 3 - x)
            } else {
                (1 + x / 2, 1 + y / 2)
            };
            assert_eq!(
                [color.r(), color.g(), color.b(), color.a()],
                [(sx * 50) as u8, (sy * 50) as u8, 17, 255],
                "local{x},{y}, device{device_point:?}"
            );
        }
    }
}

#[test]
fn varying_point_texels_preserve_rotation_crop_origin_zoom_and_reflection() {
    let rotation = [0.0, 1.0, 0.0, -1.0, 0.0, 1.0];
    let crop = [0.5, 0.0, 0.25, 0.0, 0.5, 0.25];
    for device in [
        Matrix::default(),
        Matrix::translate((9.0, 5.0)),
        Matrix::scale((2.0, 2.0)),
        Matrix::new_all(-2.0, 0.0, 60.0, 0.0, 2.0, 5.0, 0.0, 0.0, 1.0),
        Matrix::new_all(0.0, -2.0, 60.0, 2.0, 0.0, 5.0, 0.0, 0.0, 1.0),
    ] {
        point_texels(device, rotation, true);
        point_texels(device, crop, false);
    }
}
