//! Shape fill stacks preserve order/opacity while resources retain full paints.
use super::*;
use crate::layout_scene::SceneFillLayer;
pub(super) fn eligible(n: &SceneNode) -> bool {
    !n.fill_layers.is_empty()
        && matches!(
            n.kind,
            NodeKind::Rect
                | NodeKind::Frame
                | NodeKind::Ellipse
                | NodeKind::Polygon
                | NodeKind::Path
        )
        && !composite::has_widget_visual(n)
}
fn key(n: &SceneNode, i: usize) -> String {
    format!("fill-paint-{}-{i}", svg_id(&n.id))
}
pub(super) fn emit_defs(out: &mut String, n: &SceneNode) -> Result<(), SvgExportError> {
    if !eligible(n) {
        return Ok(());
    }
    for (i, layer) in n.fill_layers.iter().enumerate() {
        let id = key(n, i);
        match layer {
            SceneFillLayer::Gradient { gradient, .. } => {
                emit_gradient(out, &id, gradient, n.bounds)
            }
            SceneFillLayer::Shader { shader, .. } => {
                if let Some(paint) = &shader.gradient {
                    gradient_geometry::emit(out, &id, paint, n.bounds, shader.opacity);
                }
            }
            SceneFillLayer::Image { .. } => {
                let node = image_fields::layer_node(n, layer);
                out.push_str(
                    &image_fields::definition(&id, &node, n.bounds, n.bounds).map_err(
                        |source| SvgExportError::InvalidImage {
                            node_id: n.id.clone(),
                            source,
                        },
                    )?,
                );
            }
            _ => {}
        }
    }
    Ok(())
}
pub(super) fn emit(out: &mut String, n: &SceneNode) {
    let isolate = n.opacity < 1.0
        || n.fill_layers
            .iter()
            .any(|p| p.blend_mode() != crate::ImageBlendMode::Normal);
    if isolate {
        let _ = write!(
            out,
            r#"<g opacity="{}" style="isolation:isolate">"#,
            n.opacity
        );
    }
    for (i, layer) in n.fill_layers.iter().enumerate().rev() {
        let isolated = render::emit_composite_start(out, 1.0, layer.blend_mode());
        let resource = key(n, i);
        let attrs = match layer {
            SceneFillLayer::Solid { color, .. } => format!(
                r#"fill="{}" fill-opacity="{}""#,
                color_to_rgb(*color),
                color.a
            ),
            SceneFillLayer::Gradient { .. } | SceneFillLayer::Image { .. } => {
                format!(r#"fill="url(#{resource})""#)
            }
            SceneFillLayer::Shader { shader, .. } => {
                if shader.gradient.is_some() {
                    format!(r#"fill="url(#{resource})""#)
                } else {
                    format!(
                        r#"fill="{}" fill-opacity="{}""#,
                        color_to_rgb(shader.fallback),
                        shader.fallback.a * shader.opacity
                    )
                }
            }
        };
        stroke_paint::shape(out, n, &format!(r#"{attrs} stroke="none""#));
        if isolated {
            out.push_str("</g>");
        }
    }
    if isolate {
        out.push_str("</g>");
    }
}
