//! The exact metadata surface rewritten by replace::replace_in_node_header.
//! Keep these paired when extending the compatibility token-rewrite contract.
use super::*;
pub(crate) fn visit_reference_fields<'a>(node: &'a PenNode, visit: &mut impl FnMut(&'a str)) {
    fn value<'a>(s: &'a str, f: &mut impl FnMut(&'a str)) {
        if let Some(name) = s.strip_prefix('$') {
            f(name);
        }
    }
    fn expr<'a>(v: Option<&'a NumberOrExpression>, f: &mut impl FnMut(&'a str)) {
        if let Some(NumberOrExpression::Expression(s)) = v {
            value(s, f);
        }
    }
    expr(node.base().opacity.as_ref(), visit);
    let container = match node {
        PenNode::Frame(n) => Some(&n.container),
        PenNode::Group(n) => Some(&n.container),
        PenNode::Rectangle(n) => Some(&n.container),
        _ => None,
    };
    if let Some(c) = container {
        expr(c.gap.as_ref(), visit);
        if let Some(Padding::Expression(s)) = &c.padding {
            value(s, visit);
        }
    }
    if let Some(fills) = crate::fills::node_fills(node) {
        for fill in fills {
            fill_fields(fill, visit);
        }
    }
    if let Some(fills) = crate::fills::node_stroke_fills(node) {
        for fill in fills {
            fill_fields(fill, visit);
        }
    }
    for effect in crate::fills::node_effects(node) {
        if let PenEffect::Shadow(body) = effect {
            value(&body.color, visit);
        }
    }
    if let PenNode::Text(text) = node {
        match &text.content {
            TextContent::Plain(s) => value(s, visit),
            TextContent::Styled(segments) => {
                for s in segments {
                    if let Some(fill) = &s.fill {
                        value(fill, visit);
                    }
                    if let Some(fills) = &s.fill_paints {
                        for fill in fills {
                            fill_fields(fill, visit);
                        }
                    }
                }
            }
        }
    }
}
fn fill_fields<'a>(fill: &'a PenFill, visit: &mut impl FnMut(&'a str)) {
    let mut color = |s: &'a str| {
        if let Some(name) = s.strip_prefix('$') {
            visit(name);
        }
    };
    match fill {
        PenFill::Solid(body) => color(&body.color),
        PenFill::LinearGradient(body) => {
            for s in &body.stops {
                color(&s.color)
            }
        }
        PenFill::RadialGradient(body) => {
            for s in &body.stops {
                color(&s.color)
            }
        }
        PenFill::MeshGradient(body) => {
            for s in &body.stops {
                color(&s.color)
            }
        }
        PenFill::Shader(body) => {
            for value in body.uniforms.iter().flat_map(|u| u.values()) {
                if let ShaderUniformValue::Color(c) = value {
                    color(c);
                }
            }
        }
        PenFill::Image(_) => {}
    }
}
