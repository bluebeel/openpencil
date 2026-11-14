//! Vector SVG representation of explicit canonical gradient fields.
//!
//! SVG gradients interpolate straight alpha. A premultiplied ramp is the sum
//! of constant-color alpha-basis ramps, accumulated by SVG filter arithmetic
//! in the declared color space. No pixels, samples or raster assets are stored.

use base64::Engine;
use jian_ops_schema::style::{
    GradientAlphaInterpolation, GradientGeometry, GradientInterpolation, GradientPaint,
    GradientShape, GradientSpread,
};
use op_util::xml_escape::escape_xml as escape;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

pub const METADATA_ATTRIBUTE: &str = "data-openpencil-gradient-v1";
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SvgGradientMetadata {
    pub version: u32,
    pub bounds: [f64; 4],
    pub paint: GradientPaint,
    /// Effective scene opacity is separate from the original f64 paint value.
    pub render_opacity: f64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GradientSvgError {
    InvalidBounds,
    InvalidGeometry,
    InvalidColor { index: usize },
    InvalidStopOffset { index: usize },
    EmptyStops,
    InvalidIdentifier,
    InvalidMetadata { stage: &'static str },
}
impl std::fmt::Display for GradientSvgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBounds => {
                f.write_str("gradient bounds must be finite with positive dimensions")
            }
            Self::InvalidGeometry => f.write_str("gradient geometry or opacity is invalid"),
            Self::InvalidColor { index } => write!(f, "invalid gradient color at stop {index}"),
            Self::InvalidStopOffset { index } => {
                write!(f, "non-finite gradient offset at stop {index}")
            }
            Self::EmptyStops => f.write_str("gradient contains no stops"),
            Self::InvalidMetadata { stage } => {
                write!(f, "invalid typed SVG gradient metadata at {stage}")
            }
            Self::InvalidIdentifier => {
                f.write_str("gradient definition requires a nonempty ASCII SVG resource ID")
            }
        }
    }
}
impl std::error::Error for GradientSvgError {}

/// Also used by the explicit shader branch, so export and render decode the
/// same float colors rather than silently disagreeing on CSS named colors.
pub fn color_rgba(raw: &str) -> Option<[f64; 4]> {
    crate::svg_import::gradient_color_rgba(raw)
}

pub fn validate(paint: &GradientPaint, bounds: [f64; 4]) -> Result<(), GradientSvgError> {
    if !bounds.iter().all(|v| v.is_finite()) || bounds[2] <= 0.0 || bounds[3] <= 0.0 {
        return Err(GradientSvgError::InvalidBounds);
    }
    if !paint.opacity.is_finite()
        || !(0.0..=1.0).contains(&paint.opacity)
        || !paint.geometry.transform.iter().all(|v| v.is_finite())
    {
        return Err(GradientSvgError::InvalidGeometry);
    }
    let finite = |values: &[f64]| values.iter().all(|v| v.is_finite());
    match paint.geometry.shape {
        GradientShape::Linear { start, end } => {
            if !finite(&start) || !finite(&end) {
                return Err(GradientSvgError::InvalidGeometry);
            }
        }
        GradientShape::Radial {
            center,
            radius,
            focal,
            focal_radius,
        } => {
            if !finite(&center)
                || !finite(&focal)
                || !finite(&[radius, focal_radius])
                || radius < 0.0
                || focal_radius < 0.0
            {
                return Err(GradientSvgError::InvalidGeometry);
            }
        }
    }
    let [x, y, w, h] = bounds;
    let [a, b, c, d, e, f] = paint.geometry.transform;
    if ![w * a, h * b, w * c, h * d, x + w * e, y + h * f]
        .iter()
        .all(|v| v.is_finite())
    {
        return Err(GradientSvgError::InvalidGeometry);
    }
    if paint.stops.is_empty() {
        return Err(GradientSvgError::EmptyStops);
    }
    for (index, stop) in paint.stops.iter().enumerate() {
        if !stop.offset.is_finite() {
            return Err(GradientSvgError::InvalidStopOffset { index });
        }
        color_rgba(&stop.color).ok_or(GradientSvgError::InvalidColor { index })?;
    }
    Ok(())
}

/// Lossless typed metadata and a standard SVG visual definition. Metadata is
/// not used as a rendering substitute: the visual subtree is independently
/// generated from the same parameters and can be verified exactly on import.
pub fn definition(
    id: &str,
    paint: &GradientPaint,
    bounds: [f64; 4],
) -> Result<String, GradientSvgError> {
    definition_with_opacity(id, paint, bounds, paint.opacity)
}

pub fn definition_with_opacity(
    id: &str,
    paint: &GradientPaint,
    bounds: [f64; 4],
    opacity: f64,
) -> Result<String, GradientSvgError> {
    validate(paint, bounds)?;
    if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
        return Err(GradientSvgError::InvalidGeometry);
    }
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
    {
        return Err(GradientSvgError::InvalidIdentifier);
    }
    let metadata = SvgGradientMetadata {
        version: 1,
        bounds,
        paint: paint.clone(),
        render_opacity: opacity,
    };
    let paint = &metadata.paint;
    let json = serde_json::to_string(&metadata).expect("finite typed gradient metadata");
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes());
    let attribute = format!(" {METADATA_ATTRIBUTE}=\"{encoded}\"");
    let colors: Vec<[f64; 4]> = paint
        .stops
        .iter()
        .enumerate()
        .map(|(index, stop)| {
            color_rgba(&stop.color).ok_or(GradientSvgError::InvalidColor { index })
        })
        .collect::<Result<_, _>>()?;
    let first = colors[0];
    let alpha_varies = colors.iter().any(|color| color[3] != first[3]);
    let rgb_varies = colors.iter().any(|color| color[..3] != first[..3]);
    let active = colors.iter().find(|color| color[3] > 0.0).copied();
    let constant_active_rgb = active.is_some_and(|active| {
        colors
            .iter()
            .filter(|color| color[3] > 0.0)
            .all(|color| color[..3] == active[..3])
    });
    let mut out = String::new();
    if paint.geometry.alpha_interpolation == GradientAlphaInterpolation::Premultiplied
        && alpha_varies
        && constant_active_rgb
    {
        let color = active.expect("active constant color");
        let stops = paint
            .stops
            .iter()
            .zip(&colors)
            .map(|(stop, alpha)| {
                (
                    stop.offset,
                    format!(
                        "rgba({},{},{},{})",
                        color[0] * 255.0,
                        color[1] * 255.0,
                        color[2] * 255.0,
                        alpha[3]
                    ),
                )
            })
            .collect::<Vec<_>>();
        write_gradient(
            &mut out,
            id,
            paint.geometry.clone(),
            bounds,
            &stops,
            opacity,
            &attribute,
        );
        return Ok(out);
    }
    if paint.geometry.alpha_interpolation != GradientAlphaInterpolation::Premultiplied
        || !alpha_varies
        || !rgb_varies
    {
        write_gradient(
            &mut out,
            id,
            paint.geometry.clone(),
            bounds,
            &paint
                .stops
                .iter()
                .map(|stop| (stop.offset, stop.color.clone()))
                .collect::<Vec<_>>(),
            opacity,
            &attribute,
        );
        return Ok(out);
    }
    let [x, y, w, h] = bounds;
    let _ = write!(
        out,
        r#"<pattern id="{id}" patternUnits="userSpaceOnUse" x="{x}" y="{y}" width="{w}" height="{h}" viewBox="{x} {y} {w} {h}" preserveAspectRatio="none"{attribute}><defs>"#
    );
    let mut positions = Vec::with_capacity(paint.stops.len());
    let mut previous = 0.0_f64;
    for stop in &paint.stops {
        previous = stop.offset.clamp(0.0, 1.0).max(previous);
        positions.push(previous);
    }
    let mut layers = Vec::new();
    for (i, color) in colors.iter().enumerate() {
        if color[3] == 0.0 {
            continue;
        }
        // With repeated offsets, an interior stop shadowed on both sides has
        // an identically zero basis. Other duplicate-stop discontinuities are
        // retained through exact ordered stop pairs.
        if i > 0
            && i + 1 < positions.len()
            && positions[i - 1] == positions[i]
            && positions[i + 1] == positions[i]
        {
            continue;
        }
        let gradient_id = format!("{id}-basis-gradient-{i}");
        let layer_id = format!("{id}-basis-layer-{i}");
        let mut stops = Vec::with_capacity(3);
        let css = |alpha| {
            format!(
                "rgba({},{},{},{})",
                color[0] * 255.0,
                color[1] * 255.0,
                color[2] * 255.0,
                alpha
            )
        };
        if i > 0 {
            stops.push((positions[i - 1], css(0.0)));
        }
        stops.push((positions[i], css(color[3])));
        if i + 1 < positions.len() {
            stops.push((positions[i + 1], css(0.0)));
        }
        write_gradient(
            &mut out,
            &gradient_id,
            paint.geometry.clone(),
            bounds,
            &stops,
            opacity,
            "",
        );
        let _ = write!(
            out,
            r#"<svg id="{layer_id}" width="{w}" height="{h}" viewBox="{x} {y} {w} {h}" preserveAspectRatio="none"><rect x="{x}" y="{y}" width="{w}" height="{h}" fill="url(#{gradient_id})"/></svg>"#
        );
        layers.push(layer_id);
    }
    let space = match paint.geometry.interpolation {
        GradientInterpolation::Srgb => "sRGB",
        GradientInterpolation::LinearRgb => "linearRGB",
    };
    let filter_id = format!("{id}-basis-sum");
    let _ = write!(
        out,
        r#"<filter id="{filter_id}" filterUnits="userSpaceOnUse" primitiveUnits="userSpaceOnUse" x="{x}" y="{y}" width="{w}" height="{h}" color-interpolation-filters="{space}">"#
    );
    for (i, layer) in layers.iter().enumerate() {
        let result = if i == 0 {
            "sum0".to_string()
        } else {
            format!("term{i}")
        };
        let _ = write!(
            out,
            r##"<feImage href="#{layer}" x="{x}" y="{y}" width="{w}" height="{h}" preserveAspectRatio="none" result="{result}"/>"##
        );
        if i > 0 {
            let previous = i - 1;
            let _ = write!(
                out,
                r#"<feComposite in="sum{previous}" in2="term{i}" operator="arithmetic" k1="0" k2="1" k3="1" k4="0" result="sum{i}"/>"#
            );
        }
    }
    out.push_str("</filter></defs>");
    let _ = write!(
        out,
        r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="none" filter="url(#{filter_id})"/></pattern>"#
    );
    Ok(out)
}

fn write_gradient(
    out: &mut String,
    id: &str,
    geometry: GradientGeometry,
    bounds: [f64; 4],
    stops: &[(f64, String)],
    opacity: f64,
    attribute: &str,
) {
    // Resolve the field to user-space explicitly. Basis rectangles have a
    // different object bbox, so retaining objectBoundingBox here would change
    // the parameterization. Original units remain lossless in typed metadata.
    let [x, y, w, h] = bounds;
    let [a, b, c, d, e, f] = geometry.transform;
    let matrix = [w * a, h * b, w * c, h * d, x + w * e, y + h * f];
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
    let spread = match geometry.spread {
        GradientSpread::Pad => "pad",
        GradientSpread::Repeat => "repeat",
        GradientSpread::Reflect => "reflect",
    };
    let space = match geometry.interpolation {
        GradientInterpolation::Srgb => "sRGB",
        GradientInterpolation::LinearRgb => "linearRGB",
    };
    let _ = write!(
        out,
        r#" gradientUnits="userSpaceOnUse" gradientTransform="matrix({} {} {} {} {} {})" spreadMethod="{spread}" color-interpolation="{space}"{attribute}>"#,
        matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5]
    );
    for (offset, color) in stops {
        let _ = write!(
            out,
            r#"<stop offset="{offset}" stop-color="{}" stop-opacity="{opacity}"/>"#,
            escape(color)
        );
    }
    let _ = write!(out, "</{tag}>");
}

pub fn decode_metadata(raw: &str) -> Result<SvgGradientMetadata, GradientSvgError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw.trim())
        .map_err(|_| GradientSvgError::InvalidMetadata { stage: "encoding" })?;
    let metadata: SvgGradientMetadata = serde_json::from_slice(&bytes)
        .map_err(|_| GradientSvgError::InvalidMetadata { stage: "JSON" })?;
    if metadata.version != 1 {
        return Err(GradientSvgError::InvalidMetadata { stage: "version" });
    }
    validate(&metadata.paint, metadata.bounds)?;
    if !metadata.render_opacity.is_finite() || !(0.0..=1.0).contains(&metadata.render_opacity) {
        return Err(GradientSvgError::InvalidMetadata {
            stage: "render opacity",
        });
    }
    Ok(metadata)
}
