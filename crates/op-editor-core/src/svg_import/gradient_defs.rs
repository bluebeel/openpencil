//! Local SVG paint servers resolved against original object geometry.

use super::*;
use crate::svg_path_data::SvgPathBounds;
use jian_ops_schema::style::{
    GradientAlphaInterpolation, GradientGeometry, GradientShape, GradientSpread, GradientStop,
    GradientUnits, LinearGradientBody, RadialGradientBody,
};
use kurbo::Affine;
use std::collections::HashSet;

#[derive(Clone, Copy)]
pub(super) struct PaintSpace {
    pub source: SvgPathBounds,
    pub target: SvgPathBounds,
    pub matrix: Affine,
    pub viewport: [f64; 4],
}

pub(super) fn resolve_paint(
    resources: &SvgResources<'_>,
    raw: &str,
    opacity: f64,
    current_color: &str,
    space: PaintSpace,
) -> Option<Vec<PenFill>> {
    let raw = raw.trim();
    if raw == "none" {
        return None;
    }
    if raw.starts_with("url(") {
        let result = local_reference(raw)
            .ok_or(SvgResourceError::Invalid {
                attribute: "paint URL",
            })
            .and_then(|id| gradient(resources, id, opacity, space));
        match result {
            Ok(paint) => return paint.map(|paint| vec![paint]),
            Err(error) => tracing::warn!(%error,"SVG paint server could not be resolved"),
        }
        // A local paint URL may explicitly declare a fallback color.
        return raw
            .split_once(')')
            .and_then(|(_, fallback)| solid(fallback, opacity, current_color))
            .map(|fill| vec![fill]);
    }
    solid(raw, opacity, current_color).map(|fill| vec![fill])
}
fn solid(raw: &str, opacity: f64, current_color: &str) -> Option<PenFill> {
    let raw = if raw.trim().eq_ignore_ascii_case("currentColor") {
        current_color
    } else {
        raw
    };
    let color = colors::canonical_css(raw)?;
    Some(solid_fill_opacity(&color, opacity))
}

fn gradient(
    resources: &SvgResources<'_>,
    id: &str,
    opacity: f64,
    space: PaintSpace,
) -> Result<Option<PenFill>, SvgResourceError> {
    if let Some(entry) = resources.entries.get(id) {
        if resource_attr(
            entry.node,
            crate::svg_gradient_composition::METADATA_ATTRIBUTE,
        )
        .is_some()
        {
            match restore_gradient(resources, entry, opacity, space) {
                Ok(paint) => return Ok(paint),
                Err(error) => {
                    if entry.node.tag == "pattern" {
                        return Err(error);
                    }
                    // A modified ordinary SVG gradient can still be imported
                    // from its real visual definition; stale metadata is ignored.
                    tracing::warn!(%error,"typed gradient metadata ignored");
                }
            }
        }
    }
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut next = Some(id);
    while let Some(id) = next {
        if chain.len() >= 64 {
            return Err(SvgResourceError::DepthLimit);
        }
        if !seen.insert(id) {
            return Err(SvgResourceError::Cycle { id: id.into() });
        }
        let entry = resources
            .entries
            .get(id)
            .ok_or_else(|| SvgResourceError::Missing { id: id.into() })?;
        if entry.node.tag != "lineargradient" && entry.node.tag != "radialgradient" {
            return Err(SvgResourceError::Unsupported {
                feature: format!("{} paint server", entry.node.tag),
            });
        }
        chain.push(entry);
        next = resource_href(entry.node);
        if next.is_none()
            && (resource_attr(entry.node, "href").is_some()
                || resource_attr(entry.node, "xlink:href").is_some())
        {
            return Err(SvgResourceError::Unsupported {
                feature: "external paint-server template".into(),
            });
        }
    }
    let node = chain[0].node;
    let attr = |name| {
        chain
            .iter()
            .find_map(|entry| resource_attr(entry.node, name))
    };
    let units = match attr("gradientUnits").unwrap_or("objectBoundingBox") {
        "objectBoundingBox" => GradientUnits::ObjectBoundingBox,
        "userSpaceOnUse" => GradientUnits::UserSpaceOnUse,
        _ => {
            return Err(SvgResourceError::Invalid {
                attribute: "gradientUnits",
            })
        }
    };
    if units == GradientUnits::ObjectBoundingBox && (space.source.w <= 0.0 || space.source.h <= 0.0)
    {
        return Ok(None);
    }
    let spread = match attr("spreadMethod").unwrap_or("pad") {
        "pad" => GradientSpread::Pad,
        "repeat" => GradientSpread::Repeat,
        "reflect" => GradientSpread::Reflect,
        _ => {
            return Err(SvgResourceError::Invalid {
                attribute: "spreadMethod",
            })
        }
    };
    let gradient_transform = if let Some(raw) = attr("gradientTransform") {
        transform_list(&[("transform".into(), raw.into())]).map_err(|_| {
            SvgResourceError::Invalid {
                attribute: "gradientTransform",
            }
        })?
    } else {
        Affine::IDENTITY
    };
    let basis = if units == GradientUnits::ObjectBoundingBox {
        Affine::translate((space.source.x, space.source.y))
            * Affine::scale_non_uniform(space.source.w, space.source.h)
    } else {
        Affine::IDENTITY
    };
    let transform = Affine::scale_non_uniform(1.0 / space.target.w, 1.0 / space.target.h)
        * Affine::translate((-space.target.x, -space.target.y))
        * space.matrix
        * basis
        * gradient_transform;
    if !transform.as_coeffs().iter().all(|v| v.is_finite()) {
        return Err(SvgResourceError::Invalid {
            attribute: "gradient coordinate transform",
        });
    }
    let coordinate = |name: &'static str, default: &str, axis: usize| {
        let raw = attr(name).unwrap_or(default).trim();
        let (value, percent) = if let Some(value) = raw.strip_suffix('%') {
            (value, true)
        } else {
            (raw, false)
        };
        let value = value
            .trim_end_matches("px")
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or(SvgResourceError::Invalid { attribute: name })?;
        Ok::<f64, SvgResourceError>(if !percent {
            value
        } else if units == GradientUnits::ObjectBoundingBox {
            value * 0.01
        } else {
            let [x, y, w, h] = space.viewport;
            match axis {
                0 => x + value * 0.01 * w,
                1 => y + value * 0.01 * h,
                _ => value * 0.01 * w.hypot(h) / std::f64::consts::SQRT_2,
            }
        })
    };
    let shape = if node.tag == "lineargradient" {
        GradientShape::Linear {
            start: [coordinate("x1", "0%", 0)?, coordinate("y1", "0%", 1)?],
            end: [coordinate("x2", "100%", 0)?, coordinate("y2", "0%", 1)?],
        }
    } else {
        let center = [coordinate("cx", "50%", 0)?, coordinate("cy", "50%", 1)?];
        let radius = coordinate("r", "50%", 2)?;
        let focal = [
            if attr("fx").is_some() {
                coordinate("fx", "50%", 0)?
            } else {
                center[0]
            },
            if attr("fy").is_some() {
                coordinate("fy", "50%", 1)?
            } else {
                center[1]
            },
        ];
        let focal_radius = coordinate("fr", "0", 2)?;
        if radius < 0.0 || focal_radius < 0.0 {
            return Err(SvgResourceError::Invalid {
                attribute: "radial gradient radius",
            });
        }
        GradientShape::Radial {
            center,
            radius,
            focal,
            focal_radius,
        }
    };
    let stops_entry = chain
        .iter()
        .find(|entry| entry.node.children.iter().any(|child| child.tag == "stop"));
    let mut stops = Vec::new();
    let mut previous = 0.0_f64;
    if let Some(entry) = stops_entry {
        for stop in entry.node.children.iter().filter(|node| node.tag == "stop") {
            let style = entry.style.merged(&stop.attrs);
            let offset = resource_attr(stop, "offset")
                .and_then(svg_opacity)
                .unwrap_or(0.0)
                .clamp(0.0, 1.0)
                .max(previous);
            previous = offset;
            let raw = if style.stop_color.eq_ignore_ascii_case("currentColor") {
                &style.color
            } else {
                &style.stop_color
            };
            let mut color = colors::rgba(raw).ok_or(SvgResourceError::Invalid {
                attribute: "stop-color",
            })?;
            color[3] *= style.stop_opacity;
            stops.push(GradientStop {
                offset,
                color: colors::css(color),
            });
        }
    }
    if stops.is_empty() {
        return Ok(None);
    }
    let geometry = Some(Box::new(GradientGeometry {
        shape,
        transform: transform.as_coeffs(),
        units,
        spread,
        interpolation: chain[0].style.interpolation,
        alpha_interpolation: GradientAlphaInterpolation::Straight,
    }));
    Ok(Some(match shape {
        GradientShape::Linear { .. } => PenFill::LinearGradient(LinearGradientBody {
            geometry,
            angle: None,
            stops,
            explain: None,
            opacity: Some(opacity),
            blend_mode: None,
        }),
        GradientShape::Radial { .. } => PenFill::RadialGradient(RadialGradientBody {
            geometry,
            cx: None,
            cy: None,
            radius: None,
            stops,
            explain: None,
            opacity: Some(opacity),
            blend_mode: None,
        }),
    }))
}
