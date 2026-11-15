use super::*;
use skia_safe::PathFillType;

fn curve() -> Path {
    let mut b = PathBuilder::new();
    b.move_to((2.0, 11.0));
    b.quad_to((2.0, -9.0), (23.0, 2.0));
    b.cubic_to((41.0, 6.0), (4.0, 39.0), (2.0, 11.0));
    b.close();
    b.detach()
}

fn distance_to_segments(point: Point, path: &Path) -> f32 {
    path.iter()
        .filter(|record| record.verb() == PathVerb::Line)
        .map(|record| {
            let p = record.points();
            let (dx, dy) = (p[1].x - p[0].x, p[1].y - p[0].y);
            let t = (((point.x - p[0].x) * dx + (point.y - p[0].y) * dy) / (dx * dx + dy * dy))
                .clamp(0.0, 1.0);
            (point.x - p[0].x - t * dx).hypot(point.y - p[0].y - t * dy)
        })
        .fold(f32::INFINITY, f32::min)
}

#[test]
fn outline_curve_error_is_bounded_after_affine_zoom_rotation_and_reflection() {
    let original = curve();
    for matrix in [
        Matrix::new_identity(),
        Matrix::new_all(3.0, -2.0, 113.25, -1.5, -4.0, 24.0, 0.0, 0.0, 1.0),
        Matrix::scale((0.125, 8.0)),
    ] {
        let flattened = flatten(&original, &matrix).unwrap();
        assert!(flattened.iter().all(|record| matches!(
            record.verb(),
            PathVerb::Move | PathVerb::Line | PathVerb::Close
        )));
        let device = flattened.with_transform(&matrix);
        for i in 0..=1000 {
            let t = i as f32 / 1000.0;
            let s = 1.0 - t;
            let quadratic = Point::new(
                s * s * 2.0 + 2.0 * s * t * 2.0 + t * t * 23.0,
                s * s * 11.0 - 18.0 * s * t + t * t * 2.0,
            );
            let cubic = Point::new(
                s * s * s * 23.0 + 3.0 * s * s * t * 41.0 + 3.0 * s * t * t * 4.0 + t * t * t * 2.0,
                s * s * s * 2.0 + 18.0 * s * s * t + 117.0 * s * t * t + 11.0 * t * t * t,
            );
            for point in [quadratic, cubic] {
                assert!(
                    distance_to_segments(matrix.map_point(point), &device)
                        <= MAX_DEVICE_ERROR + 0.0001,
                    "device error at {t}"
                );
            }
        }
    }
}

#[test]
fn outline_flattening_preserves_winding_holes_and_open_contours() {
    for fill in [PathFillType::Winding, PathFillType::EvenOdd] {
        let mut b = PathBuilder::new_with_fill_type(fill);
        b.move_to((0.0, 0.0));
        b.line_to((20.0, 0.0));
        b.line_to((20.0, 20.0));
        b.line_to((0.0, 20.0));
        b.close();
        b.move_to((5.0, 5.0));
        b.line_to((5.0, 15.0));
        b.quad_to((15.0, 15.0), (15.0, 5.0));
        // Deliberately open: filling still closes this contour implicitly.
        let original = b.detach();
        let matrix = Matrix::new_all(-2.0, 0.5, 48.0, 0.25, 3.0, 2.0, 0.0, 0.0, 1.0);
        let result = flatten(&original, &matrix).unwrap();
        assert_eq!(result.fill_type(), fill);
        assert_eq!(
            original
                .iter()
                .filter(|r| r.verb() == PathVerb::Close)
                .count(),
            result
                .iter()
                .filter(|r| r.verb() == PathVerb::Close)
                .count()
        );
        for point in [(2.0, 2.0), (8.0, 8.0), (18.0, 18.0), (22.0, 5.0)] {
            assert_eq!(original.contains(point), result.contains(point));
            assert_eq!(
                original
                    .with_transform(&matrix)
                    .contains(matrix.map_point(point)),
                result
                    .with_transform(&matrix)
                    .contains(matrix.map_point(point))
            );
        }
        assert!(!result.contains((8.0, 8.0)), "hole remains empty");
    }
}

#[test]
fn outline_subdivision_budget_and_unsupported_transforms_retain_fallback() {
    let path = curve();
    assert!(flatten_with_limit(&path, &Matrix::scale((2.0, 2.0)), 8).is_none());
    assert!(flatten(
        &path,
        &Matrix::new_all(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.1, 0.0, 1.0)
    )
    .is_none());
    assert!(flatten(&path, &Matrix::scale((f32::INFINITY, 1.0))).is_none());
    let conic = Path::circle((10.0, 10.0), 8.0, None);
    assert!(flatten(&conic, &Matrix::new_identity()).is_none());
}
