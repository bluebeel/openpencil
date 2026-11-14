//! SVG endpoint arcs, shared by exact bounds and editable cubic conversion.

use kurbo::{Arc, Point, Vec2};
use std::f64::consts::{PI, TAU};

/// SVG endpoint-to-center conversion. Zero radii and coincident endpoints are
/// handled by the caller, as required by SVG. This avoids the curve library's
/// small-radius straight-line threshold: finite nonzero arcs remain curves.
#[allow(clippy::too_many_arguments)]
pub(crate) fn endpoint_arc(
    from: (f64, f64),
    to: (f64, f64),
    rx: f64,
    ry: f64,
    rotation_degrees: f64,
    large: bool,
    sweep: bool,
) -> Option<Arc> {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 || from == to {
        return None;
    }
    let phi = rotation_degrees.rem_euclid(360.0).to_radians();
    let (sin, cos) = phi.sin_cos();
    let (hx, hy) = (from.0 * 0.5 - to.0 * 0.5, from.1 * 0.5 - to.1 * 0.5);
    let (px, py) = (cos * hx + sin * hy, -sin * hx + cos * hy);
    let correction = (px / rx).hypot(py / ry);
    if correction > 1.0 {
        rx *= correction;
        ry *= correction;
    }
    let (ux, uy) = (px / rx, py / ry);
    let length = ux.hypot(uy);
    if !length.is_finite() || length == 0.0 {
        return None;
    }
    let sign = if large == sweep { -1.0 } else { 1.0 };
    let factor = sign * (1.0 - length * length).max(0.0).sqrt() / length;
    let (cx, cy) = (factor * rx * uy, -factor * ry * ux);
    let center = Point::new(
        cos * cx - sin * cy + from.0 * 0.5 + to.0 * 0.5,
        sin * cx + cos * cy + from.1 * 0.5 + to.1 * 0.5,
    );
    let start = ((py - cy) / ry).atan2((px - cx) / rx);
    let end = ((-py - cy) / ry).atan2((-px - cx) / rx);
    let mut angle = (end - start).rem_euclid(TAU);
    if !sweep && angle > 0.0 {
        angle -= TAU;
    }
    if ![center.x, center.y, rx, ry, start, angle]
        .iter()
        .all(|v| v.is_finite())
    {
        return None;
    }
    Some(Arc::new(center, Vec2::new(rx, ry), start, angle, phi))
}

pub(crate) fn include_arc_bounds(arc: &Arc, include: &mut impl FnMut(f64, f64)) {
    let (sin, cos) = arc.x_rotation.sin_cos();
    let x_extreme = (-arc.radii.y * sin).atan2(arc.radii.x * cos);
    let y_extreme = (arc.radii.y * cos).atan2(arc.radii.x * sin);
    for angle in [x_extreme, x_extreme + PI, y_extreme, y_extreme + PI] {
        let travel = if arc.sweep_angle >= 0.0 {
            (angle - arc.start_angle).rem_euclid(TAU)
        } else {
            (arc.start_angle - angle).rem_euclid(TAU)
        };
        if travel <= arc.sweep_angle.abs() {
            let (s, c) = angle.sin_cos();
            include(
                arc.center.x + arc.radii.x * c * cos - arc.radii.y * s * sin,
                arc.center.y + arc.radii.x * c * sin + arc.radii.y * s * cos,
            );
        }
    }
}
