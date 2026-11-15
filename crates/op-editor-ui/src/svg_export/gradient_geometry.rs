//! Standard vector SVG composition for all explicit gradient alpha policies.
use super::*;
use jian_ops_schema::style::GradientPaint;
use op_editor_core::svg_gradient_composition as composition;

pub(super) fn emit(out: &mut String, id: &str, paint: &GradientPaint, bounds: Rect, opacity: f32) {
    let bounds = [
        f64::from(bounds.origin.x),
        f64::from(bounds.origin.y),
        f64::from(bounds.size.x),
        f64::from(bounds.size.y),
    ];
    // The serializer validates the same immutable scene first. No raster
    // fallback or partial SVG is emitted for malformed explicit parameters.
    let definition = composition::definition_with_opacity(id, paint, bounds, f64::from(opacity))
        .expect("validated immutable gradient definition");
    out.push_str(&definition);
}

pub(super) fn validate(node: &SceneNode) -> Result<(), SvgExportError> {
    if node.hidden {
        return Ok(());
    }
    if let Some(shader) = &node.shader {
        if let Some(paint) = &shader.gradient {
            let bounds = normalize_rect(node.bounds);
            let bounds = [
                f64::from(bounds.origin.x),
                f64::from(bounds.origin.y),
                f64::from(bounds.size.x),
                f64::from(bounds.size.y),
            ];
            composition::validate(paint, bounds).map_err(|source| {
                SvgExportError::InvalidGradient {
                    node_id: node.id.clone(),
                    source,
                }
            })?;
            if !shader.opacity.is_finite() || !(0.0..=1.0).contains(&shader.opacity) {
                return Err(SvgExportError::InvalidGradient {
                    node_id: node.id.clone(),
                    source: composition::GradientSvgError::InvalidGeometry,
                });
            }
        }
    }
    for layer in &node.fill_layers {
        if let crate::layout_scene::SceneFillLayer::Shader { shader, .. } = layer {
            if let Some(paint) = &shader.gradient {
                let b = normalize_rect(node.bounds);
                composition::validate(
                    paint,
                    [
                        b.origin.x as f64,
                        b.origin.y as f64,
                        b.size.x as f64,
                        b.size.y as f64,
                    ],
                )
                .map_err(|source| SvgExportError::InvalidGradient {
                    node_id: node.id.clone(),
                    source,
                })?;
            }
        }
    }
    if let Some(stroke) = &node.stroke_paint {
        for layer in &stroke.layers {
            if let crate::layout_scene::SceneFillLayer::Shader { shader, .. } = layer {
                if let Some(paint) = &shader.gradient {
                    if !shader.opacity.is_finite() || !(0.0..=1.0).contains(&shader.opacity) {
                        return Err(SvgExportError::InvalidGradient {
                            node_id: node.id.clone(),
                            source: composition::GradientSvgError::InvalidGeometry,
                        });
                    }
                    let mut b = normalize_rect(node.bounds);
                    if b.size.x == 0.0 {
                        b.size.x = 1.0;
                    }
                    if b.size.y == 0.0 {
                        b.size.y = 1.0;
                    }
                    composition::validate(
                        paint,
                        [
                            b.origin.x as f64,
                            b.origin.y as f64,
                            b.size.x as f64,
                            b.size.y as f64,
                        ],
                    )
                    .map_err(|source| SvgExportError::InvalidGradient {
                        node_id: node.id.clone(),
                        source,
                    })?;
                }
            }
        }
    }
    for child in node.visible_children() {
        validate(child)?;
    }
    Ok(())
}
