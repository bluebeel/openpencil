//! Preserve typed affine/focal gradients through the SVG transport.

use super::*;
use jian_ops_schema::style::{
    GradientInterpolation, GradientPaint, GradientShape, GradientSpread, GradientUnits,
};

pub(super) fn emit(out: &mut String, id: &str, paint: &GradientPaint, bounds: Rect, opacity: f32) {
    let geometry = &paint.geometry;
    let [a, b, c, d, e, f] = geometry.transform;
    let (units, matrix) = match geometry.units {
        GradientUnits::ObjectBoundingBox => ("objectBoundingBox", [a, b, c, d, e, f]),
        GradientUnits::UserSpaceOnUse => {
            let (w, h) = (f64::from(bounds.size.x), f64::from(bounds.size.y));
            (
                "userSpaceOnUse",
                [
                    w * a,
                    h * b,
                    w * c,
                    h * d,
                    f64::from(bounds.origin.x) + w * e,
                    f64::from(bounds.origin.y) + h * f,
                ],
            )
        }
    };
    let spread = match geometry.spread {
        GradientSpread::Pad => "pad",
        GradientSpread::Repeat => "repeat",
        GradientSpread::Reflect => "reflect",
    };
    let interpolation = match geometry.interpolation {
        GradientInterpolation::Srgb => "sRGB",
        GradientInterpolation::LinearRgb => "linearRGB",
    };
    let transform = format!(
        "matrix({} {} {} {} {} {})",
        matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5]
    );
    let tag = match geometry.shape {
        GradientShape::Linear { start, end } => {
            let _ = write!(
                out,
                r#"<linearGradient id="{id}" x1="{}" y1="{}" x2="{}" y2="{}""#,
                start[0], start[1], end[0], end[1]
            );
            "linearGradient"
        }
        GradientShape::Radial {
            center,
            radius,
            focal,
            focal_radius,
        } => {
            let _ = write!(
                out,
                r#"<radialGradient id="{id}" cx="{}" cy="{}" r="{radius}" fx="{}" fy="{}" fr="{focal_radius}""#,
                center[0], center[1], focal[0], focal[1]
            );
            "radialGradient"
        }
    };
    let _ = write!(
        out,
        r#" gradientUnits="{units}" gradientTransform="{transform}" spreadMethod="{spread}" color-interpolation="{interpolation}">"#
    );
    for stop in &paint.stops {
        let _ = write!(
            out,
            r#"<stop offset="{}" stop-color="{}" stop-opacity="{}"/>"#,
            stop.offset,
            xml_escape(&stop.color),
            opacity
        );
    }
    let _ = write!(out, "</{tag}>");
}

pub(super) fn validate(node: &SceneNode) -> Result<(), SvgExportError> {
    if node.hidden {
        return Ok(());
    }
    if let Some(shader) = &node.shader {
        if let Some(paint) = &shader.gradient {
            if paint.geometry.alpha_interpolation
                == jian_ops_schema::style::GradientAlphaInterpolation::Premultiplied
            {
                let mut colors = shader
                    .uniforms
                    .iter()
                    .filter(|u| {
                        u.name.starts_with('c')
                            && u.name[1..].bytes().all(|b| b.is_ascii_digit())
                            && u.values.len() == 4
                    })
                    .map(|u| &u.values);
                if let Some(first) = colors.next() {
                    let mut alpha_varies = false;
                    let mut rgb_varies = false;
                    for color in colors {
                        alpha_varies |= color[3] != first[3];
                        rgb_varies |= color[..3] != first[..3];
                    }
                    if alpha_varies && rgb_varies {
                        return Err(SvgExportError::UnsupportedGradientInterpolation {
                            node_id: node.id.clone(),
                        });
                    }
                }
            }
        }
    }
    for child in node.visible_children() {
        validate(child)?;
    }
    Ok(())
}
