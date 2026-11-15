//! Resolve a single image through inherited group/use wrappers without flattening media.
use super::*;
use kurbo::Affine;
pub(super) struct ImageContent<'a> {
    pub node: &'a SvgTree,
    pub context: StyleCtx,
    pub transform: Affine,
    pub opacity: f64,
}
pub(super) fn single_image<'a>(
    nodes: &'a [SvgTree],
    parent: &StyleCtx,
    matrix: Affine,
    opacity: f64,
    resources: &SvgResources<'a>,
) -> Result<Option<ImageContent<'a>>, SvgResourceError> {
    let mut found = None;
    for node in nodes.iter().filter(|n| !is_definition(&n.tag)) {
        if extract_style_or_attr(&node.attrs, "display").as_deref() == Some("none") {
            continue;
        }
        let context = merge_style_ctx(parent, &node.attrs);
        let transform = matrix
            * transform_list(&node.attrs).map_err(|_| SvgResourceError::Invalid {
                attribute: "pattern child transform",
            })?;
        let child_opacity = match svg_node_opacity(&node.attrs) {
            None => 1.0,
            Some(jian_ops_schema::node::base::NumberOrExpression::Number(value)) => value,
            Some(_) => {
                return Err(SvgResourceError::Invalid {
                    attribute: "pattern child opacity",
                })
            }
        };
        let opacity = opacity * child_opacity;
        let child = match node.tag.as_str() {
            "image" => Some(ImageContent {
                node,
                context,
                transform,
                opacity,
            }),
            "g" => {
                if ["clip-path", "mask", "filter"]
                    .iter()
                    .any(|key| extract_style_or_attr(&node.attrs, key).is_some_and(|v| v != "none"))
                {
                    return Err(SvgResourceError::Unsupported {
                        feature: "image pattern group clip/mask/filter".into(),
                    });
                }
                single_image(&node.children, &context, transform, opacity, resources)?
            }
            "use" => {
                let id = resource_href(node).ok_or(SvgResourceError::Invalid {
                    attribute: "pattern use href",
                })?;
                let _scope = resources.enter(id)?;
                let entry = resources
                    .entries
                    .get(id)
                    .ok_or_else(|| SvgResourceError::Missing { id: id.into() })?;
                let offset = Affine::translate((
                    svg_number(node.attr("x").unwrap_or("0")).unwrap_or(0.0),
                    svg_number(node.attr("y").unwrap_or("0")).unwrap_or(0.0),
                ));
                single_image(
                    std::slice::from_ref(entry.node),
                    &context,
                    transform * offset,
                    opacity,
                    resources,
                )?
            }
            _ => {
                return Err(SvgResourceError::Unsupported {
                    feature: "multi-paint/vector pattern content".into(),
                })
            }
        };
        if let Some(child) = child {
            if found.is_some() {
                return Err(SvgResourceError::Unsupported {
                    feature: "multi-image pattern content".into(),
                });
            }
            found = Some(child);
        }
    }
    Ok(found)
}
