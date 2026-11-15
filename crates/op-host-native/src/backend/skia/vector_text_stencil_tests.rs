use super::*;
use skia_safe::{BlendMode, Color, Color4f, Paint, PathBuilder, PathDirection};

fn rectangle() -> Path {
    let mut builder = PathBuilder::new();
    builder.add_rect(Rect::from_xywh(2.0, 3.0, 8.0, 7.0), None, None);
    builder.detach()
}

#[test]
fn stencil_projected_extent_does_not_gain_a_row_from_vertex_snapping() {
    let path = Path::rect(Rect::from_xywh(2.0, 0.501, 8.0, 7.0), None);
    let pixels = render(&path, Matrix::default(), 0, false, 1.0);
    assert_eq!(pixels[4], [1.0; 4], "row center precedes the primitive top");
    assert_eq!(pixels[32 + 4], [1.0, 0.0, 0.0, 1.0]);
}

#[test]
fn stencil_projected_extent_is_unchanged_by_a_disjoint_curved_contour() {
    let path = Path::rect(Rect::from_xywh(2.0, 0.501, 8.0, 7.0), None);
    let mut builder = PathBuilder::new();
    builder.add_path(&path, None);
    builder.move_to((1000.0, -1000.0));
    builder.quad_to((1004.0, -1010.0), (1008.0, -1000.0));
    builder.close();
    let disjoint = builder.detach();
    for grid in [0, 1, 2] {
        assert_eq!(
            render(&path, Matrix::default(), grid, false, 1.0),
            render(&disjoint, Matrix::default(), grid, false, 1.0),
            "grid {grid}"
        );
    }
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

#[test]
fn stencil_boundary_centers_follow_device_top_left_after_reflection_and_rotation() {
    let path = Path::rect(Rect::from_xywh(2.5, 3.5, 8.0, 7.0), None);
    for matrix in [
        Matrix::default(),
        Matrix::translate((5., 7.)),
        Matrix::new_all(-1., 0., 24., 0., 1., 2., 0., 0., 1.),
        Matrix::new_all(0., -1., 24., 1., 0., 2., 0., 0., 1.),
    ] {
        let (bounds, _) = matrix.map_rect(path.bounds());
        let pixels = render(&path, matrix, 0, false, 0.5);
        for y in 0..32 {
            for x in 0..32 {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let inside = px >= bounds.left
                    && px < bounds.right
                    && py >= bounds.top
                    && py < bounds.bottom;
                assert_eq!(
                    pixels[y * 32 + x],
                    if inside { [1., 0.5, 0.5, 1.] } else { [1.; 4] },
                    "{matrix:?}, {x},{y}"
                );
            }
        }
    }
}

#[test]
fn stencil_fractional_rectangle_matches_independent_quantized_half_open_edges() {
    let rect = Rect::from_xywh(2.46875, 3.3125, 8.375, 4.75);
    let path = Path::rect(rect, None);
    for grid in [1, 2] {
        let pixels = render(&path, Matrix::default(), grid, false, 0.5);
        for y in 0..32 {
            for x in 0..32 {
                let mut count = 0;
                for (jx, jy) in JITTERS {
                    let q = |v: f32| (v * 16.).round_ties_even();
                    let left = q(grid as f32 * rect.left - jx);
                    let right = q(grid as f32 * rect.right - jx);
                    let top = q(grid as f32 * rect.top - jy);
                    let bottom = q(grid as f32 * rect.bottom - jy);
                    for dy in 0..grid {
                        for dx in 0..grid {
                            let px = (x as u32 * grid + dx) as f32 * 16.;
                            let py = (y as u32 * grid + dy) as f32 * 16.;
                            count +=
                                u32::from(px >= left && px < right && py >= top && py < bottom);
                        }
                    }
                }
                let alpha = count as f32 / (4 * grid * grid) as f32;
                assert_eq!(
                    pixels[y * 32 + x],
                    [1., 1. - alpha * 0.5, 1. - alpha * 0.5, 1.],
                    "{grid}, {x},{y}"
                );
            }
        }
    }
}

#[test]
fn stencil_quadratic_hulls_choose_short_local_diagonals_and_keep_degenerate_split() {
    let curve = |start: Point, control: Point, end: Point| {
        let mut path = PathBuilder::new();
        path.move_to(start);
        path.quad_to(control, end);
        path.close();
        triangles(&path.detach())
            .unwrap()
            .into_iter()
            .filter(|t| !t.interior)
            .map(|t| t.values)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        curve((0., 0.).into(), (0., 6.).into(), (12., 0.).into()),
        vec![
            [CUBIC[0], CUBIC[1], CUBIC[2]],
            [CUBIC[0], CUBIC[2], CUBIC[3]]
        ]
    );
    assert_eq!(
        curve((12., 0.).into(), (0., 6.).into(), (0., 0.).into()),
        vec![
            [CUBIC[0], CUBIC[1], CUBIC[3]],
            [CUBIC[1], CUBIC[2], CUBIC[3]]
        ]
    );
    for control in [(6., 6.), (5., 0.)] {
        assert_eq!(
            curve((0., 0.).into(), control.into(), (12., 0.).into()),
            vec![
                [CUBIC[0], CUBIC[1], CUBIC[3]],
                [CUBIC[1], CUBIC[2], CUBIC[3]]
            ]
        );
    }
}
