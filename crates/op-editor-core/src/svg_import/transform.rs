//! SVG column-vector affine composition. Geometry is baked into leaves because
//! canonical groups do not expose a general shear/nonuniform matrix.

use super::*;
use kurbo::Affine;
use svgtypes::{NumberListParser, TransformListParser, TransformListToken};

#[derive(Debug)]
pub(super) enum SvgTransformError {
    Syntax(svgtypes::Error),
    NonFinite,
    InvalidViewport { attribute: &'static str },
}
impl std::fmt::Display for SvgTransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax(error) => std::fmt::Display::fmt(error, f),
            Self::NonFinite => f.write_str("SVG transform produces non-finite geometry"),
            Self::InvalidViewport { attribute } => write!(f, "invalid SVG {attribute}"),
        }
    }
}
impl std::error::Error for SvgTransformError {}

pub(super) fn transform_list(attrs: &[(String, String)]) -> Result<Affine, SvgTransformError> {
    let Some(raw) = attr(attrs, "transform") else {
        return Ok(Affine::IDENTITY);
    };
    let mut matrix = Affine::IDENTITY;
    for token in TransformListParser::from(raw) {
        let next = match token.map_err(SvgTransformError::Syntax)? {
            TransformListToken::Matrix { a, b, c, d, e, f } => Affine::new([a, b, c, d, e, f]),
            TransformListToken::Translate { tx, ty } => Affine::translate((tx, ty)),
            TransformListToken::Scale { sx, sy } => Affine::scale_non_uniform(sx, sy),
            TransformListToken::Rotate { angle } => {
                Affine::rotate(angle.rem_euclid(360.0).to_radians())
            }
            TransformListToken::SkewX { angle } => {
                Affine::new([1.0, 0.0, angle.to_radians().tan(), 1.0, 0.0, 0.0])
            }
            TransformListToken::SkewY { angle } => {
                Affine::new([1.0, angle.to_radians().tan(), 0.0, 1.0, 0.0, 0.0])
            }
        };
        // SVG lists post-multiply: translate(10) scale(2) maps x to 2x+10.
        matrix *= next;
        finite(matrix)?;
    }
    Ok(matrix)
}

pub(super) fn root_transform(
    attrs: &[(String, String)],
    offset: (f64, f64),
) -> Result<Affine, SvgTransformError> {
    let viewbox = viewbox(attrs)?;
    let width = length(attrs, "width", viewbox.map(|b| b[2]).unwrap_or(100.0))?;
    let height = length(attrs, "height", viewbox.map(|b| b[3]).unwrap_or(100.0))?;
    let normalization = (SVG_MAX_DIM / width.max(height)).min(1.0);
    let matrix = Affine::translate(offset)
        * Affine::scale(normalization)
        * transform_list(attrs)?
        * viewbox_matrix(attrs, viewbox, width, height)?;
    finite(matrix)?;
    Ok(matrix)
}

pub(super) fn element_transform(el: &SvgTree, parent: Affine) -> Result<Affine, SvgTransformError> {
    let mut matrix = parent * transform_list(&el.attrs)?;
    if el.tag == "svg" {
        let vb = viewbox(&el.attrs)?;
        let x = coordinate(&el.attrs, "x")?;
        let y = coordinate(&el.attrs, "y")?;
        matrix *= Affine::translate((x, y));
        if let Some(vb) = vb {
            let width = length(&el.attrs, "width", vb[2])?;
            let height = length(&el.attrs, "height", vb[3])?;
            matrix *= viewbox_matrix(&el.attrs, Some(vb), width, height)?;
        }
    }
    finite(matrix)?;
    Ok(matrix)
}

pub(super) fn similarity_scale(matrix: Affine) -> Option<f64> {
    let [a, b, c, d, _, _] = matrix.as_coeffs();
    let (x, y) = (a.hypot(b), c.hypot(d));
    // Exact classification: do not discard even a small authored shear.
    (x > 0.0 && x == y && a * c + b * d == 0.0).then_some(x)
}

pub(super) fn finite(matrix: Affine) -> Result<(), SvgTransformError> {
    if matrix.as_coeffs().iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(SvgTransformError::NonFinite)
    }
}
fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}
fn coordinate(attrs: &[(String, String)], name: &'static str) -> Result<f64, SvgTransformError> {
    let Some(raw) = attr(attrs, name) else {
        return Ok(0.0);
    };
    raw.trim()
        .trim_end_matches("px")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or(SvgTransformError::InvalidViewport { attribute: name })
}
fn length(
    attrs: &[(String, String)],
    name: &'static str,
    default: f64,
) -> Result<f64, SvgTransformError> {
    let Some(raw) = attr(attrs, name) else {
        return Ok(default);
    };
    // No external CSS viewport exists at import. Percent dimensions retain the
    // declared viewBox fallback rather than inventing a containing viewport.
    if raw.trim().ends_with('%') {
        return Ok(default);
    }
    let value = coordinate(attrs, name)?;
    if value > 0.0 {
        Ok(value)
    } else {
        Err(SvgTransformError::InvalidViewport { attribute: name })
    }
}
fn viewbox(attrs: &[(String, String)]) -> Result<Option<[f64; 4]>, SvgTransformError> {
    let Some(raw) = attr(attrs, "viewBox") else {
        return Ok(None);
    };
    let mut numbers = NumberListParser::from(raw);
    let mut values = [0.0; 4];
    for value in &mut values {
        *value = numbers
            .next()
            .ok_or(SvgTransformError::InvalidViewport {
                attribute: "viewBox",
            })?
            .map_err(SvgTransformError::Syntax)?;
    }
    if numbers.next().is_some()
        || !values.iter().all(|v| v.is_finite())
        || values[2] <= 0.0
        || values[3] <= 0.0
    {
        return Err(SvgTransformError::InvalidViewport {
            attribute: "viewBox",
        });
    }
    Ok(Some(values))
}
fn viewbox_matrix(
    attrs: &[(String, String)],
    vb: Option<[f64; 4]>,
    width: f64,
    height: f64,
) -> Result<Affine, SvgTransformError> {
    let Some([x, y, w, h]) = vb else {
        return Ok(Affine::IDENTITY);
    };
    let (sx, sy) = (width / w, height / h);
    let raw = attr(attrs, "preserveAspectRatio").unwrap_or("xMidYMid meet");
    let mut tokens = raw.split_whitespace().filter(|s| *s != "defer");
    let align = tokens.next().unwrap_or("xMidYMid");
    if align == "none" {
        return Ok(Affine::scale_non_uniform(sx, sy) * Affine::translate((-x, -y)));
    }
    let mode = tokens.next().unwrap_or("meet");
    let scale = match mode {
        "meet" => sx.min(sy),
        "slice" => sx.max(sy),
        _ => {
            return Err(SvgTransformError::InvalidViewport {
                attribute: "preserveAspectRatio",
            })
        }
    };
    let align_axis = |part: &str| match part {
        "Min" => Some(0.0),
        "Mid" => Some(0.5),
        "Max" => Some(1.0),
        _ => None,
    };
    if align.len() != 8
        || !align.starts_with('x')
        || !align.is_char_boundary(4)
        || !align[4..].starts_with('Y')
    {
        return Err(SvgTransformError::InvalidViewport {
            attribute: "preserveAspectRatio",
        });
    }
    let ax = align_axis(&align[1..4]).ok_or(SvgTransformError::InvalidViewport {
        attribute: "preserveAspectRatio",
    })?;
    let ay = align_axis(&align[5..]).ok_or(SvgTransformError::InvalidViewport {
        attribute: "preserveAspectRatio",
    })?;
    Ok(
        Affine::translate(((width - w * scale) * ax, (height - h * scale) * ay))
            * Affine::scale(scale)
            * Affine::translate((-x, -y)),
    )
}

pub(super) fn source_viewport(
    attrs: &[(String, String)],
    parent: [f64; 4],
) -> Result<[f64; 4], SvgTransformError> {
    if let Some(viewbox) = viewbox(attrs)? {
        return Ok(viewbox);
    }
    Ok([
        0.0,
        0.0,
        length(attrs, "width", parent[2])?,
        length(attrs, "height", parent[3])?,
    ])
}
