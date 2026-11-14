use super::*;
use skia_safe::{BlendMode, Color, Color4f, Paint, PathBuilder, PathDirection};

fn rectangle() -> Path {
    let mut builder = PathBuilder::new();
    builder.add_rect(Rect::from_xywh(2.0, 3.0, 8.0, 7.0), None, None);
    builder.detach()
}

fn render(path: &Path, matrix: Matrix, grid: u32, clipped: bool, alpha: f32) -> Vec<[f32; 4]> {
    let info = ImageInfo::new((32, 32), ColorType::RGBAF32, AlphaType::Premul, None);
    let mut surface = skia_safe::surfaces::raster(&info, None, None).unwrap();
    let canvas = surface.canvas();
    canvas.clear(Color::WHITE);
    if clipped {
        canvas.clip_rect(Rect::from_xywh(0.0, 0.0, 16.0, 32.0), None, false);
    }
    canvas.concat(&matrix);
    let (shader, bounds) =
        coverage(path, &matrix, canvas.device_clip_bounds().unwrap(), grid).unwrap();
    canvas.clip_shader(shader, skia_safe::ClipOp::Intersect);
    let mut paint = Paint::default();
    paint.set_color4f(Color4f::new(1.0, 0.0, 0.0, alpha), None);
    paint.set_blend_mode(BlendMode::SrcOver);
    canvas.draw_rect(bounds, &paint);
    surface
        .peek_pixels()
        .unwrap()
        .bytes()
        .unwrap()
        .chunks_exact(16)
        .map(|p| {
            std::array::from_fn(|c| f32::from_ne_bytes(p[c * 4..c * 4 + 4].try_into().unwrap()))
        })
        .collect()
}

#[test]
fn stencil_integer_spans_equal_exact_top_left_half_planes() {
    for points in [
        [[-83, -90], [167, -113], [92, 211]],
        [[-64, 0], [128, 0], [0, 192]],
        [[0, -128], [0, 192], [128, 0]],
    ] {
        for p in [points, [points[2], points[1], points[0]]] {
            let orientation = cross(p[0], p[1], p[2]);
            for y in -24..24 {
                let interval = span(p, y, orientation, -24, 24);
                for x in -24..24 {
                    assert_eq!(
                        interval.is_some_and(|(a, b)| x >= a && x < b),
                        inside(p, [i64::from(x) * 16, -i64::from(y) * 16], orientation),
                        "{p:?}, {x},{y}"
                    );
                }
            }
        }
    }
}

#[test]
fn stencil_point_rectangles_keep_translation_zoom_rotation_and_reflection() {
    for matrix in [
        Matrix::default(),
        Matrix::translate((5.0, 7.0)),
        Matrix::scale((2.0, 2.0)),
        Matrix::new_all(-1.0, 0.0, 24.0, 0.0, 1.0, 2.0, 0.0, 0.0, 1.0),
        Matrix::new_all(0.0, -1.0, 24.0, 1.0, 0.0, 2.0, 0.0, 0.0, 1.0),
    ] {
        let pixels = render(&rectangle(), matrix, 0, false, 0.5);
        let inverse = matrix.invert().unwrap();
        for y in 0..32 {
            for x in 0..32 {
                let p = map(&inverse, Point::new(x as f32 + 0.5, y as f32 + 0.5));
                let expected = if (2.0..10.0).contains(&p.x) && (3.0..10.0).contains(&p.y) {
                    [1.0, 0.5, 0.5, 1.0]
                } else {
                    [1.0; 4]
                };
                assert_eq!(pixels[y * 32 + x], expected, "{matrix:?}, {x},{y}");
            }
        }
    }
}

#[test]
fn stencil_preserves_winding_and_even_odd_holes() {
    let mut builder = PathBuilder::new();
    builder.add_rect(
        Rect::from_xywh(2.0, 2.0, 24.0, 24.0),
        PathDirection::CW,
        None,
    );
    builder.add_rect(Rect::from_xywh(8.0, 8.0, 8.0, 8.0), PathDirection::CW, None);
    let path = builder.detach();
    for (fill, expected) in [
        (PathFillType::Winding, [1.0, 0.0, 0.0, 1.0]),
        (PathFillType::EvenOdd, [1.0; 4]),
    ] {
        let mut builder = PathBuilder::new_with_fill_type(fill);
        builder.add_path(&path, None);
        let pixels = render(&builder.detach(), Matrix::default(), 0, false, 1.0);
        assert_eq!(pixels[12 * 32 + 12], expected);
        assert_eq!(pixels[4 * 32 + 4], [1.0, 0.0, 0.0, 1.0]);
    }
}

#[test]
fn stencil_fractional_coverage_keeps_float_alpha_and_clipping_once() {
    let mut builder = PathBuilder::new();
    builder.move_to((2.0, 18.0));
    builder.quad_to((11.0, -5.0), (27.0, 19.0));
    builder.line_to((27.0, 27.0));
    builder.line_to((2.0, 27.0));
    builder.close();
    let path = builder.detach();
    let full = render(&path, Matrix::default(), 2, false, 0.5);
    let clipped = render(&path, Matrix::default(), 2, true, 0.5);
    let mut fractional = false;
    for y in 0..32 {
        for x in 0..32 {
            let p = full[y * 32 + x];
            assert_eq!(p[0], 1.0);
            assert_eq!(p[3], 1.0);
            let count = (1.0 - p[1]) * 32.0;
            assert!((0.0..=16.0).contains(&count));
            assert_eq!(count, count.round());
            fractional |= count > 0.0 && count < 16.0;
            assert_eq!(clipped[y * 32 + x], if x < 16 { p } else { [1.0; 4] });
        }
    }
    assert!(
        fractional,
        "the quadratic must exercise fractional binary16 coverage"
    );
}

#[test]
fn stencil_declines_unsupported_paths_and_bounds_before_large_allocations() {
    let clip = IRect::from_wh(8192, 8192);
    let mut cubic = PathBuilder::new();
    cubic.move_to((0.0, 0.0));
    cubic.cubic_to((2.0, 3.0), (6.0, 3.0), (8.0, 0.0));
    cubic.close();
    assert!(coverage(&cubic.detach(), &Matrix::default(), clip, 2).is_none());
    assert!(coverage(&rectangle(), &Matrix::scale((0.0, 1.0)), clip, 2).is_none());
    let perspective = Matrix::new_all(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.01, 0.0, 1.0);
    assert!(coverage(&rectangle(), &perspective, clip, 2).is_none());
    let mut huge = PathBuilder::new();
    huge.add_rect(Rect::from_wh(4096.0, 4096.0), None, None);
    assert!(coverage(&huge.detach(), &Matrix::default(), clip, 2).is_none());
}
