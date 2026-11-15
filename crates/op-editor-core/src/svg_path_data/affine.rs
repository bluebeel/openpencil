//! Bake an SVG affine map into path geometry, retaining ellipse arc primitives
//! for nonsingular maps and f64 cubic handles for rank-deficient maps.

use super::{endpoint_arc, localize_segments, AbsoluteSegments, Segment, SvgPathBounds};
use kurbo::{Affine, BezPath, PathEl, Point};

pub(crate) fn localize_transformed(d: &str, matrix: Affine) -> Option<(String, SvgPathBounds)> {
    if matrix == Affine::IDENTITY {
        return super::localize_svg_path(d);
    }
    localize_segments(&transformed_segments(d, matrix)?)
}

pub(crate) fn transformed_bounds(d: &str, matrix: Affine) -> Option<SvgPathBounds> {
    if matrix == Affine::IDENTITY {
        return super::svg_path_bounds(d);
    }
    super::segment_bounds(&transformed_segments(d, matrix)?)
}

fn transformed_segments(d: &str, matrix: Affine) -> Option<Vec<Segment>> {
    let mut out = Vec::new();
    let mut current = (0.0, 0.0);
    let mut start = current;
    let mut cubic = None;
    let mut quadratic = None;
    let reflect = |previous: Option<(f64, f64)>, p: (f64, f64)| {
        previous
            .map(|q| (p.0 + (p.0 - q.0), p.1 + (p.1 - q.1)))
            .unwrap_or(p)
    };
    let point = |x, y| {
        let p = matrix * Point::new(x, y);
        (p.x, p.y)
    };
    for segment in AbsoluteSegments::new(d) {
        let segment = segment.ok()?;
        let previous_cubic = cubic.take();
        let previous_quad = quadratic.take();
        let transformed = match segment {
            Segment::Move(x, y) => {
                start = (x, y);
                let p = point(x, y);
                Segment::Move(p.0, p.1)
            }
            Segment::Line(x, y) => {
                let p = point(x, y);
                Segment::Line(p.0, p.1)
            }
            Segment::HLine(x) => {
                let p = point(x, current.1);
                Segment::Line(p.0, p.1)
            }
            Segment::VLine(y) => {
                let p = point(current.0, y);
                Segment::Line(p.0, p.1)
            }
            Segment::Cubic(a, b, c, d, x, y) => {
                cubic = Some((c, d));
                let p = point(a, b);
                let q = point(c, d);
                let r = point(x, y);
                Segment::Cubic(p.0, p.1, q.0, q.1, r.0, r.1)
            }
            Segment::SmoothCubic(a, b, x, y) => {
                let first = reflect(previous_cubic, current);
                cubic = Some((a, b));
                let p = point(first.0, first.1);
                let q = point(a, b);
                let r = point(x, y);
                Segment::Cubic(p.0, p.1, q.0, q.1, r.0, r.1)
            }
            Segment::Quad(a, b, x, y) => {
                quadratic = Some((a, b));
                let p = point(a, b);
                let q = point(x, y);
                Segment::Quad(p.0, p.1, q.0, q.1)
            }
            Segment::SmoothQuad(x, y) => {
                let control = reflect(previous_quad, current);
                quadratic = Some(control);
                let p = point(control.0, control.1);
                let q = point(x, y);
                Segment::Quad(p.0, p.1, q.0, q.1)
            }
            Segment::Arc(rx, ry, rotation, large, sweep, x, y) => {
                let end = point(x, y);
                if current == (x, y) {
                    Segment::Arc(0.0, 0.0, 0.0, large, sweep, end.0, end.1)
                } else if rx == 0.0 || ry == 0.0 {
                    Segment::Line(end.0, end.1)
                } else {
                    let arc = endpoint_arc(
                        current,
                        (x, y),
                        rx,
                        ry,
                        rotation,
                        large != 0.0,
                        sweep != 0.0,
                    )?;
                    if let Some((rx, ry, rotation, reflected)) = ellipse_axes(matrix, &arc) {
                        Segment::Arc(
                            rx,
                            ry,
                            rotation,
                            large,
                            if reflected { 1.0 - sweep } else { sweep },
                            end.0,
                            end.1,
                        )
                    } else {
                        // A singular map can collapse an arc into a line that
                        // reverses direction. A single degenerate A would lose
                        // that traversal, so preserve it as transformed cubics.
                        let tolerance = (arc.radii.x.max(arc.radii.y) * 1e-9).max(1e-7);
                        let mut curves = arc.append_iter(tolerance).peekable();
                        while let Some(PathEl::CurveTo(p, q, r)) = curves.next() {
                            let p = matrix * p;
                            let q = matrix * q;
                            let r = matrix * r;
                            let end = if curves.peek().is_none() {
                                end
                            } else {
                                (r.x, r.y)
                            };
                            out.push(Segment::Cubic(p.x, p.y, q.x, q.y, end.0, end.1));
                        }
                        current = (x, y);
                        continue;
                    }
                }
            }
            Segment::Close => Segment::Close,
        };
        current = match segment {
            Segment::Move(x, y)
            | Segment::Line(x, y)
            | Segment::SmoothQuad(x, y)
            | Segment::Quad(_, _, x, y)
            | Segment::SmoothCubic(_, _, x, y)
            | Segment::Cubic(_, _, _, _, x, y)
            | Segment::Arc(_, _, _, _, _, x, y) => (x, y),
            Segment::HLine(x) => (x, current.1),
            Segment::VLine(y) => (current.0, y),
            Segment::Close => start,
        };
        out.push(transformed);
    }
    Some(out)
}

/// Principal axes of A R(phi) diag(rx,ry), obtained from its normalized
/// covariance. Use determinant/sigma_max for sigma_min to avoid cancellation
/// for strongly nonuniform but nonsingular transforms.
fn ellipse_axes(matrix: Affine, arc: &kurbo::Arc) -> Option<(f64, f64, f64, bool)> {
    let [a, b, c, d, _, _] = matrix.as_coeffs();
    let (sin, cos) = arc.x_rotation.sin_cos();
    let e = [
        (a * cos + c * sin) * arc.radii.x,
        (b * cos + d * sin) * arc.radii.x,
        (-a * sin + c * cos) * arc.radii.y,
        (-b * sin + d * cos) * arc.radii.y,
    ];
    let size = e.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
    if !size.is_finite() || size == 0.0 {
        return None;
    }
    let [a, b, c, d] = e.map(|v| v / size);
    let det = a * d - b * c;
    if det == 0.0 {
        return None;
    }
    let (xx, xy, yy) = (a * a + c * c, a * b + c * d, b * b + d * d);
    let major = ((xx + yy + (xx - yy).hypot(2.0 * xy)) * 0.5).sqrt();
    let minor = det.abs() / major;
    let (rx, ry) = (major * size, minor * size);
    if !rx.is_finite() || !ry.is_finite() || ry == 0.0 {
        return None;
    }
    Some((
        rx,
        ry,
        (0.5 * (2.0 * xy).atan2(xx - yy)).to_degrees(),
        det < 0.0,
    ))
}

/// Cubic geometry for stroke expansion only. The normal import retains A.
pub(crate) fn cubic_path(d: &str) -> Option<BezPath> {
    let mut out = BezPath::new();
    let mut current = (0.0, 0.0);
    let mut start = current;
    let mut cubic = None;
    let mut quadratic = None;
    let reflect = |previous: Option<(f64, f64)>, p: (f64, f64)| {
        previous
            .map(|q| (p.0 + (p.0 - q.0), p.1 + (p.1 - q.1)))
            .unwrap_or(p)
    };
    for segment in AbsoluteSegments::new(d) {
        let segment = segment.ok()?;
        let previous_cubic = cubic.take();
        let previous_quad = quadratic.take();
        match segment {
            Segment::Move(x, y) => {
                out.move_to((x, y));
                current = (x, y);
                start = current;
            }
            Segment::Line(x, y) => {
                out.line_to((x, y));
                current = (x, y);
            }
            Segment::HLine(x) => {
                current.0 = x;
                out.line_to(current);
            }
            Segment::VLine(y) => {
                current.1 = y;
                out.line_to(current);
            }
            Segment::Cubic(a, b, c, d, x, y) => {
                out.curve_to((a, b), (c, d), (x, y));
                current = (x, y);
                cubic = Some((c, d));
            }
            Segment::SmoothCubic(a, b, x, y) => {
                out.curve_to(reflect(previous_cubic, current), (a, b), (x, y));
                current = (x, y);
                cubic = Some((a, b));
            }
            Segment::Quad(a, b, x, y) => {
                out.quad_to((a, b), (x, y));
                current = (x, y);
                quadratic = Some((a, b));
            }
            Segment::SmoothQuad(x, y) => {
                let q = reflect(previous_quad, current);
                out.quad_to(q, (x, y));
                current = (x, y);
                quadratic = Some(q);
            }
            Segment::Arc(rx, ry, rotation, large, sweep, x, y) => {
                if current != (x, y) {
                    if rx == 0.0 || ry == 0.0 {
                        out.line_to((x, y));
                    } else {
                        let arc = endpoint_arc(
                            current,
                            (x, y),
                            rx,
                            ry,
                            rotation,
                            large != 0.0,
                            sweep != 0.0,
                        )?;
                        let tolerance = (arc.radii.x.max(arc.radii.y) * 1e-9).max(1e-7);
                        let mut curves = arc.append_iter(tolerance).peekable();
                        while let Some(PathEl::CurveTo(p, q, r)) = curves.next() {
                            let r = if curves.peek().is_none() {
                                Point::new(x, y)
                            } else {
                                r
                            };
                            out.curve_to(p, q, r);
                        }
                    }
                }
                current = (x, y);
            }
            Segment::Close => {
                out.close_path();
                current = start;
            }
        }
    }
    if out.elements().iter().all(|e| e.is_finite()) {
        Some(out)
    } else {
        None
    }
}
