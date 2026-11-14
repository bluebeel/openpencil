//! SVG presentation attributes: style-context inheritance, fill /
//! stroke resolution and the named-colour table.

use super::*;

/// Merge `parent` with the element's own `fill` / `stroke` /
/// `stroke-width` (inline `style="..."` takes precedence over the
/// matching attribute, matching `extractStyleOrAttr` in TS).
pub(super) fn merge_style_ctx(parent: &StyleCtx, attrs: &[(String, String)]) -> StyleCtx {
    let value = |name: &str| extract_style_or_attr(attrs, name);
    let number = |name: &str, default: f64| {
        value(name)
            .as_deref()
            .and_then(svg_number)
            .unwrap_or(default)
    };
    StyleCtx {
        fill: value("fill")
            .filter(|v| v != "inherit")
            .or_else(|| parent.fill.clone()),
        stroke: value("stroke")
            .filter(|v| v != "inherit")
            .or_else(|| parent.stroke.clone()),
        stroke_width: number("stroke-width", parent.stroke_width).max(0.0),
        current_color: value("color")
            .filter(|v| v != "inherit")
            .unwrap_or_else(|| parent.current_color.clone()),
        clip_mode: parent.clip_mode,
        fill_rule: value(if parent.clip_mode {
            "clip-rule"
        } else {
            "fill-rule"
        })
        .as_deref()
        .and_then(parse_svg_fill_rule)
        .or(parent.fill_rule),
        stroke_cap: match value("stroke-linecap").as_deref() {
            Some("butt") => kurbo::Cap::Butt,
            Some("round") => kurbo::Cap::Round,
            Some("square") => kurbo::Cap::Square,
            _ => parent.stroke_cap,
        },
        stroke_join: match value("stroke-linejoin").as_deref() {
            Some("miter") => kurbo::Join::Miter,
            Some("round") => kurbo::Join::Round,
            Some("bevel") => kurbo::Join::Bevel,
            _ => parent.stroke_join,
        },
        stroke_miter: number("stroke-miterlimit", parent.stroke_miter).max(1.0),
        stroke_dashes: value("stroke-dasharray")
            .as_deref()
            .and_then(svg_dash_array)
            .unwrap_or_else(|| parent.stroke_dashes.clone()),
        stroke_dash_offset: number("stroke-dashoffset", parent.stroke_dash_offset),
        fill_opacity: value("fill-opacity")
            .as_deref()
            .and_then(svg_opacity)
            .unwrap_or(parent.fill_opacity)
            .clamp(0.0, 1.0),
        stroke_opacity: value("stroke-opacity")
            .as_deref()
            .and_then(svg_opacity)
            .unwrap_or(parent.stroke_opacity)
            .clamp(0.0, 1.0),
    }
}

pub(super) fn parse_svg_fill_rule(raw: &str) -> Option<PathFillRule> {
    match raw.trim() {
        value if value.eq_ignore_ascii_case("evenodd") => Some(PathFillRule::Evenodd),
        value if value.eq_ignore_ascii_case("nonzero") => Some(PathFillRule::Nonzero),
        _ => None,
    }
}

/// Look up `name` in an inline `style="..."` first, then fall back
/// to the named attribute. Mirrors `extractStyleOrAttr` from the TS
/// regex parser.
pub(super) fn extract_style_or_attr(attrs: &[(String, String)], name: &str) -> Option<String> {
    if let Some(style) = attrs.iter().find(|(k, _)| k == "style").map(|(_, v)| v) {
        // Naive CSS-ish split — values can't contain `;` because
        // SVG inline styles forbid it.
        for pair in style.split(';') {
            let trimmed = pair.trim();
            if let Some((k, v)) = trimmed.split_once(':') {
                if k.trim().eq_ignore_ascii_case(name) {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.to_string())
}

pub(super) fn resolve_svg_fill_hex(attrs: &[(String, String)], ctx: &StyleCtx) -> Option<String> {
    let raw = extract_style_or_attr(attrs, "fill").or_else(|| ctx.fill.clone());
    match raw.as_deref().map(str::trim) {
        None => Some("#000000".to_string()),
        Some(v) if v.eq_ignore_ascii_case("none") || v.eq_ignore_ascii_case("transparent") => None,
        Some(v) if v.to_ascii_lowercase().starts_with("url(") => Some("#000000".to_string()),
        Some(v) => parse_svg_color(v),
    }
}

pub(super) fn resolve_svg_stroke(
    attrs: &[(String, String)],
    ctx: &StyleCtx,
    scale: f64,
) -> Option<PenStroke> {
    let raw = extract_style_or_attr(attrs, "stroke").or_else(|| ctx.stroke.clone())?;
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case("none") || raw.to_ascii_lowercase().starts_with("url(") {
        return None;
    }
    let hex = parse_svg_color(raw).unwrap_or_else(|| "#000000".to_string());
    let width = (ctx.stroke_width * scale).max(0.0);
    if width <= 0.0 {
        return None;
    }
    if !width.is_finite()
        || !(ctx.stroke_dash_offset * scale).is_finite()
        || ctx
            .stroke_dashes
            .iter()
            .any(|dash| !(dash * scale).is_finite())
    {
        tracing::warn!("SVG stroke scale produces non-finite paint parameters");
        return None;
    }
    Some(PenStroke::from(jian_ops_schema::style::PenStrokeBody {
        thickness: StrokeThickness::Uniform(width),
        align: None,
        join: Some(match ctx.stroke_join {
            kurbo::Join::Miter => jian_ops_schema::style::StrokeJoin::Miter,
            kurbo::Join::Round => jian_ops_schema::style::StrokeJoin::Round,
            kurbo::Join::Bevel => jian_ops_schema::style::StrokeJoin::Bevel,
        }),
        cap: Some(match ctx.stroke_cap {
            kurbo::Cap::Butt => jian_ops_schema::style::StrokeCap::None,
            kurbo::Cap::Round => jian_ops_schema::style::StrokeCap::Round,
            kurbo::Cap::Square => jian_ops_schema::style::StrokeCap::Square,
        }),
        dash_pattern: if ctx.stroke_dashes.is_empty() {
            None
        } else {
            Some(ctx.stroke_dashes.iter().map(|v| v * scale).collect())
        },
        dash_offset: Some(ctx.stroke_dash_offset * scale),
        fill: Some(vec![solid_fill_opacity(&hex, ctx.stroke_opacity)]),
    }))
}

pub(super) fn solid_fill(hex: &str) -> PenFill {
    PenFill::Solid(SolidFillBody {
        color: hex.to_string(),
        explain: None,
        opacity: None,
        blend_mode: None,
    })
}

pub(super) fn set_node_stroke(node: &mut PenNode, stroke: PenStroke) {
    match node {
        PenNode::Frame(n) => n.container.stroke = Some(stroke),
        PenNode::Group(n) => n.container.stroke = Some(stroke),
        PenNode::Rectangle(n) => n.container.stroke = Some(stroke),
        PenNode::Ellipse(n) => n.stroke = Some(stroke),
        PenNode::Polygon(n) => n.stroke = Some(stroke),
        PenNode::Path(n) => n.stroke = Some(stroke),
        PenNode::Line(n) => n.stroke = Some(stroke),
        PenNode::TextInput(n) => n.stroke = Some(stroke),
        PenNode::TextArea(n) => n.stroke = Some(stroke),
        PenNode::Select(n) => n.stroke = Some(stroke),
        PenNode::Switch(n) => n.stroke = Some(stroke),
        PenNode::Checkbox(n) => n.stroke = Some(stroke),
        PenNode::Slider(n) => n.stroke = Some(stroke),
        PenNode::RadioGroup(n) => n.stroke = Some(stroke),
        PenNode::NumberInput(n) => n.stroke = Some(stroke),
        PenNode::Progress(n) => n.stroke = Some(stroke),
        PenNode::Tabs(n) => n.stroke = Some(stroke),
        PenNode::Text(_) | PenNode::IconFont(_) | PenNode::Image(_) | PenNode::Ref(_) => {}
    }
}

pub(super) fn clear_node_fill(node: &mut PenNode) {
    match node {
        PenNode::Frame(n) => n.container.fill = None,
        PenNode::Group(n) => n.container.fill = None,
        PenNode::Rectangle(n) => n.container.fill = None,
        PenNode::Ellipse(n) => n.fill = None,
        PenNode::Polygon(n) => n.fill = None,
        PenNode::Path(n) => n.fill = None,
        PenNode::Text(n) => n.fill = None,
        PenNode::TextInput(n) => n.fill = None,
        PenNode::IconFont(n) => n.fill = None,
        PenNode::TextArea(n) => n.fill = None,
        PenNode::Select(n) => n.fill = None,
        PenNode::Switch(n) => n.fill = None,
        PenNode::Checkbox(n) => n.fill = None,
        PenNode::Slider(n) => n.fill = None,
        PenNode::RadioGroup(n) => n.fill = None,
        PenNode::NumberInput(n) => n.fill = None,
        PenNode::Progress(n) => n.fill = None,
        PenNode::Tabs(n) => n.fill = None,
        PenNode::Line(_) | PenNode::Image(_) | PenNode::Ref(_) => {}
    }
}

/// Preserve functional RGB and alpha without packing imported float colors.
/// Invalid colors/none return no paint; transparent remains an explicit color.
pub(super) fn parse_svg_color(raw: &str) -> Option<String> {
    colors::canonical_css(raw)
}

pub(super) fn svg_number(raw: &str) -> Option<f64> {
    raw.trim()
        .trim_end_matches("px")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}
pub(super) fn svg_opacity(raw: &str) -> Option<f64> {
    if let Some(raw) = raw.trim().strip_suffix('%') {
        svg_number(raw).map(|v| v * 0.01)
    } else {
        svg_number(raw)
    }
}

fn svg_dash_array(raw: &str) -> Option<Vec<f64>> {
    if raw.trim() == "none" {
        return Some(Vec::new());
    }
    let mut values: Vec<f64> = svgtypes::NumberListParser::from(raw)
        .collect::<Result<_, _>>()
        .ok()?;
    if values.is_empty() || values.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return None;
    }
    if values.iter().all(|v| *v == 0.0) {
        return Some(Vec::new());
    }
    if values.len() % 2 != 0 {
        values.extend_from_within(..);
    }
    Some(values)
}

pub(super) fn solid_fill_opacity(hex: &str, opacity: f64) -> PenFill {
    let mut fill = solid_fill(hex);
    if let PenFill::Solid(body) = &mut fill {
        body.opacity = if opacity == 1.0 { None } else { Some(opacity) };
    }
    fill
}

pub(super) fn svg_node_opacity(
    attrs: &[(String, String)],
) -> Option<jian_ops_schema::node::base::NumberOrExpression> {
    extract_style_or_attr(attrs, "opacity")
        .as_deref()
        .and_then(svg_opacity)
        .map(|v| jian_ops_schema::node::base::NumberOrExpression::Number(v.clamp(0.0, 1.0)))
}
