//! Keep complete normalized Figma gradient geometry rather than just an angle.

use super::*;
use jian_ops_schema::style::{
    GradientAlphaInterpolation, GradientGeometry, GradientInterpolation, GradientShape,
    GradientSpread, GradientUnits,
};

pub(super) fn map_gradient_geometry(
    paint: &FigValue,
    radial: bool,
) -> Option<Box<GradientGeometry>> {
    // The serialized paint transform maps normalized node positions into
    // gradient coordinates (the historical angle projection negated rotation).
    let transform = if let Some(matrix) = paint.get("transform").and_then(FigMatrix::from_value) {
        inverse([
            matrix.m00, matrix.m10, matrix.m01, matrix.m11, matrix.m02, matrix.m12,
        ])?
    } else {
        let handles = paint.get_array("gradientHandlePositions")?;
        if handles.len() < 3 {
            return None;
        }
        let point = |i: usize| Some([handles.get(i)?.get_f64("x")?, handles.get(i)?.get_f64("y")?]);
        let (p, q, r) = (point(0)?, point(1)?, point(2)?);
        let factor = if radial { 2.0 } else { 1.0 };
        let u = [(q[0] - p[0]) * factor, (q[1] - p[1]) * factor];
        let v = [(r[0] - p[0]) * 2.0, (r[1] - p[1]) * 2.0];
        let origin = if radial {
            [p[0] - (u[0] + v[0]) * 0.5, p[1] - (u[1] + v[1]) * 0.5]
        } else {
            [p[0] - v[0] * 0.5, p[1] - v[1] * 0.5]
        };
        [u[0], u[1], v[0], v[1], origin[0], origin[1]]
    };
    if !transform.iter().all(|v| v.is_finite()) {
        return None;
    }
    Some(Box::new(GradientGeometry {
        shape: if radial {
            GradientShape::Radial {
                center: [0.5, 0.5],
                radius: 0.5,
                focal: [0.5, 0.5],
                focal_radius: 0.0,
            }
        } else {
            GradientShape::Linear {
                start: [0.0, 0.5],
                end: [1.0, 0.5],
            }
        },
        transform,
        units: GradientUnits::ObjectBoundingBox,
        spread: GradientSpread::Pad,
        interpolation: GradientInterpolation::Srgb,
        alpha_interpolation: GradientAlphaInterpolation::Premultiplied,
    }))
}
fn inverse([a, b, c, d, e, f]: [f64; 6]) -> Option<[f64; 6]> {
    let det = a * d - b * c;
    if !det.is_finite() || det == 0.0 {
        return None;
    }
    let out = [
        d / det,
        -b / det,
        -c / det,
        a / det,
        (c * f - d * e) / det,
        (b * e - a * f) / det,
    ];
    out.iter().all(|v| v.is_finite()).then_some(out)
}
