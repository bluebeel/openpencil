//! Request-local borrowed SVG definition index. Definitions never paint as
//! ordinary siblings; only local fragment references can instantiate them.

use super::*;
use std::{cell::RefCell, collections::HashMap};

#[derive(Debug)]
pub(super) enum SvgResourceError {
    Missing { id: String },
    Cycle { id: String },
    DepthLimit,
    Invalid { attribute: &'static str },
    Unsupported { feature: String },
}
impl std::fmt::Display for SvgResourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { id } => write!(f, "missing local SVG definition #{id}"),
            Self::Cycle { id } => write!(f, "cyclic SVG definition #{id}"),
            Self::DepthLimit => f.write_str("SVG reference depth exceeds 64"),
            Self::Invalid { attribute } => write!(f, "invalid SVG resource {attribute}"),
            Self::Unsupported { feature } => write!(f, "unsupported SVG resource {feature}"),
        }
    }
}
impl std::error::Error for SvgResourceError {}

#[derive(Clone)]
pub(super) struct DefinitionStyle {
    pub color: String,
    pub stop_color: String,
    pub stop_opacity: f64,
    pub interpolation: jian_ops_schema::style::GradientInterpolation,
}
impl Default for DefinitionStyle {
    fn default() -> Self {
        Self {
            color: "black".into(),
            stop_color: "black".into(),
            stop_opacity: 1.0,
            interpolation: Default::default(),
        }
    }
}
impl DefinitionStyle {
    pub fn merged(&self, attrs: &[(String, String)]) -> Self {
        let color = extract_style_or_attr(attrs, "color")
            .filter(|v| v != "inherit")
            .unwrap_or_else(|| self.color.clone());
        let stop_color = extract_style_or_attr(attrs, "stop-color")
            .filter(|v| v != "inherit")
            .unwrap_or_else(|| self.stop_color.clone());
        let stop_opacity = extract_style_or_attr(attrs, "stop-opacity")
            .as_deref()
            .and_then(svg_opacity)
            .unwrap_or(self.stop_opacity)
            .clamp(0.0, 1.0);
        let interpolation = match extract_style_or_attr(attrs, "color-interpolation").as_deref() {
            Some("linearRGB") => jian_ops_schema::style::GradientInterpolation::LinearRgb,
            Some("sRGB") => jian_ops_schema::style::GradientInterpolation::Srgb,
            _ => self.interpolation,
        };
        Self {
            color,
            stop_color,
            stop_opacity,
            interpolation,
        }
    }
}
pub(super) struct Definition<'a> {
    pub node: &'a SvgTree,
    pub style: DefinitionStyle,
    pub context: StyleCtx,
}
pub(super) struct SvgResources<'a> {
    pub entries: HashMap<&'a str, Definition<'a>>,
    active: RefCell<Vec<&'a str>>,
}
impl<'a> SvgResources<'a> {
    pub fn new(nodes: &'a [SvgTree], root_attrs: &[(String, String)]) -> Self {
        fn walk<'a>(
            nodes: &'a [SvgTree],
            style: &DefinitionStyle,
            context: &StyleCtx,
            out: &mut HashMap<&'a str, Definition<'a>>,
        ) {
            for node in nodes {
                let style = style.merged(&node.attrs);
                let context = merge_style_ctx(context, &node.attrs);
                if let Some(id) = node.attr("id") {
                    out.entry(id).or_insert_with(|| Definition {
                        node,
                        style: style.clone(),
                        context: context.clone(),
                    });
                }
                walk(&node.children, &style, &context, out);
            }
        }
        let mut entries = HashMap::new();
        walk(
            nodes,
            &DefinitionStyle::default().merged(root_attrs),
            &merge_style_ctx(&StyleCtx::default(), root_attrs),
            &mut entries,
        );
        Self {
            entries,
            active: RefCell::new(Vec::new()),
        }
    }
    pub fn enter(&self, id: &str) -> Result<ResourceScope<'_, 'a>, SvgResourceError> {
        let (key, _) = self
            .entries
            .get_key_value(id)
            .ok_or_else(|| SvgResourceError::Missing { id: id.into() })?;
        let mut active = self.active.borrow_mut();
        if active.len() >= 64 {
            return Err(SvgResourceError::DepthLimit);
        }
        if active.contains(key) {
            return Err(SvgResourceError::Cycle { id: id.into() });
        }
        active.push(*key);
        Ok(ResourceScope { resources: self })
    }
}
pub(super) struct ResourceScope<'r, 'a> {
    resources: &'r SvgResources<'a>,
}
impl Drop for ResourceScope<'_, '_> {
    fn drop(&mut self) {
        self.resources.active.borrow_mut().pop();
    }
}

pub(super) fn local_reference(raw: &str) -> Option<&str> {
    let raw = raw.trim();
    let inner = if let Some(raw) = raw.strip_prefix("url(") {
        raw.split_once(')')?.0.trim().trim_matches(['\'', '"'])
    } else {
        raw
    };
    inner.strip_prefix('#').filter(|id| !id.is_empty())
}
pub(super) fn resource_attr<'a>(node: &'a SvgTree, name: &str) -> Option<&'a str> {
    node.attrs
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}
pub(super) fn resource_href(node: &SvgTree) -> Option<&str> {
    resource_attr(node, "href")
        .or_else(|| resource_attr(node, "xlink:href"))
        .and_then(local_reference)
}
