//! Vector text paint fields. All paints share one vector alpha-mask geometry
//! for a slice; inherited fields remain anchored to the original text node.
use super::*;
use crate::layout_scene::SceneFillLayer;
use crate::ImageBlendMode;
use std::collections::BTreeSet;
pub(in crate::svg_export) mod coverage;
mod geometry;
const MAX_GEOMETRY_BYTES: usize = 8 * 1024 * 1024;
fn invalid(n: &SceneNode) -> SvgExportError {
    SvgExportError::InvalidStyledText {
        node_id: n.id.clone(),
    }
}
fn limit(n: &SceneNode) -> SvgExportError {
    SvgExportError::StyledTextLimit {
        node_id: n.id.clone(),
        limit: MAX_GEOMETRY_BYTES,
    }
}
fn rich(layers: &[SceneFillLayer]) -> bool {
    !layers.is_empty()
        && !matches!(
            layers,
            [SceneFillLayer::Solid {
                blend_mode: ImageBlendMode::Normal,
                ..
            }]
        )
}
pub(super) fn eligible(n: &SceneNode) -> bool {
    // New explicit stacks opt into the vector run path even when empty or a
    // single solid. Legacy scenes with only the old Color fields stay intact.
    matches!(n.kind, NodeKind::Text)
        && (rich(&n.fill_layers) || n.text_runs.iter().any(|run| run.fill_layers.is_some()))
}

#[derive(Clone, Copy)]
enum Paint<'a> {
    None,
    Solid(Color),
    Layers(&'a [SceneFillLayer], Option<usize>),
}
fn paint<'a>(n: &'a SceneNode, s: geometry::Slice) -> Paint<'a> {
    let run = s.run.map(|i| &n.text_runs[i]);
    if run.is_some_and(|r| r.fill_disabled) {
        return Paint::None;
    }
    let legacy = run
        .and_then(|r| r.fill)
        .unwrap_or(n.fill.unwrap_or(TEXT_DEFAULT_FILL));
    if let Some(layers) = run.and_then(|r| r.fill_layers.as_deref()) {
        if layers.is_empty() {
            return Paint::None;
        }
        return if rich(layers) {
            Paint::Layers(layers, s.run)
        } else {
            Paint::Solid(legacy)
        };
    }
    if run.and_then(|r| r.fill).is_some() {
        return Paint::Solid(legacy);
    }
    if n.fill_disabled {
        Paint::None
    } else if rich(&n.fill_layers) {
        Paint::Layers(&n.fill_layers, None)
    } else {
        Paint::Solid(legacy)
    }
}
fn shape_key(n: &SceneNode, slice: usize) -> String {
    format!("text-shape-{}-{slice}", svg_id(&n.id))
}
fn mask_key(n: &SceneNode, slice: usize) -> String {
    format!("text-mask-{}-{slice}", svg_id(&n.id))
}
fn paint_key(n: &SceneNode, run: Option<usize>, layer: usize) -> String {
    let owner = run.map_or_else(|| "node".into(), |i| format!("run{i}"));
    format!("text-paint-{}-{owner}-{layer}", svg_id(&n.id))
}
fn rect(out: &mut String, r: Rect, attrs: &str) {
    let _ = write!(
        out,
        r#"<rect x="{}" y="{}" width="{}" height="{}" {attrs}/>"#,
        r.origin.x, r.origin.y, r.size.x, r.size.y
    );
}
pub(super) fn emit_defs(out: &mut String, n: &SceneNode) -> Result<(), SvgExportError> {
    if !eligible(n) {
        return Ok(());
    }
    if !n.letter_spacing.is_finite() || !n.opacity.is_finite() {
        return Err(invalid(n));
    }
    let slices = geometry::slices(n)?;
    let features = geometry::features(n)?;
    let area = coverage::area().ok_or_else(|| invalid(n))?;
    let mut shapes = geometry::Limited(String::new());
    let mut owners = BTreeSet::new();
    for (i, slice) in slices.iter().copied().enumerate() {
        let p = paint(n, slice);
        if matches!(p, Paint::None) {
            continue;
        }
        geometry::definition(&mut shapes, n, &slices, i, &shape_key(n, i), &features)
            .map_err(|_| limit(n))?;
        if let Paint::Layers(layers, owner) = p {
            let mask = mask_key(n, i);
            let shape = shape_key(n, i);
            let _ = write!(out,r#"<mask id="{mask}" maskUnits="userSpaceOnUse" maskContentUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}" mask-type="alpha" style="mask-type:alpha"><use href="#{shape}" color="white"/></mask>"#,
                area.origin.x,area.origin.y,area.size.x,area.size.y);
            if owners.insert(owner) {
                for (layer_index, layer) in layers.iter().enumerate() {
                    layer_def(out, n, layer, &paint_key(n, owner, layer_index), area)?;
                }
            }
        }
    }
    out.push_str(&shapes.0);
    Ok(())
}
fn layer_def(
    out: &mut String,
    n: &SceneNode,
    layer: &SceneFillLayer,
    id: &str,
    area: Rect,
) -> Result<(), SvgExportError> {
    let unsupported = |kind| SvgExportError::UnsupportedStyledTextPaint {
        node_id: n.id.clone(),
        kind,
    };
    match layer {
        SceneFillLayer::Solid { color, .. } => {
            if ![color.r, color.g, color.b, color.a]
                .into_iter()
                .all(f32::is_finite)
            {
                return Err(invalid(n));
            }
        }
        SceneFillLayer::Gradient {
            gradient: SceneGradient::Mesh { .. },
            ..
        } => return Err(unsupported("mesh gradient")),
        SceneFillLayer::Gradient { gradient, .. } => {
            let (numbers, stops) = match gradient {
                SceneGradient::Linear {
                    angle_deg,
                    opacity,
                    stops,
                } => ([*angle_deg, *opacity, 0.0, 0.0], stops),
                SceneGradient::Radial {
                    cx,
                    cy,
                    radius,
                    opacity,
                    stops,
                } => ([*cx, *cy, *radius, *opacity], stops),
                SceneGradient::Mesh { .. } => unreachable!(),
            };
            if !numbers.into_iter().all(f32::is_finite)
                || stops.iter().any(|s| {
                    ![s.offset, s.color.r, s.color.g, s.color.b, s.color.a]
                        .into_iter()
                        .all(f32::is_finite)
                })
            {
                return Err(invalid(n));
            }
            emit_gradient(out, id, gradient, n.bounds);
        }
        SceneFillLayer::Shader { shader, .. } => {
            let Some(paint) = shader.gradient.as_deref() else {
                return Err(unsupported("arbitrary SkSL shader"));
            };
            let b = normalize_rect(n.bounds);
            op_editor_core::svg_gradient_composition::validate(
                paint,
                [
                    b.origin.x as f64,
                    b.origin.y as f64,
                    b.size.x as f64,
                    b.size.y as f64,
                ],
            )
            .map_err(|source| SvgExportError::InvalidGradient {
                node_id: n.id.clone(),
                source,
            })?;
            if !shader.opacity.is_finite() || !(0.0..=1.0).contains(&shader.opacity) {
                return Err(invalid(n));
            }
            gradient_geometry::emit(out, id, paint, b, shader.opacity);
        }
        SceneFillLayer::Image { .. } => {
            let image = image_fields::layer_node(n, layer);
            out.push_str(
                &image_fields::definition(id, &image, n.bounds, area).map_err(|source| {
                    SvgExportError::InvalidImage {
                        node_id: n.id.clone(),
                        source,
                    }
                })?,
            );
        }
    }
    Ok(())
}
pub(super) fn emit(out: &mut String, n: &SceneNode) {
    if let Err(error) = emit_inner(out, n) {
        resource_scope::record(error);
    }
}
fn emit_inner(out: &mut String, n: &SceneNode) -> Result<(), SvgExportError> {
    let slices = geometry::slices(n)?;
    let area = coverage::area().ok_or_else(|| invalid(n))?;
    let _ = write!(out, r#"<g id="{}">"#, xml_escape(&node_id(&n.id)));
    for (i, slice) in slices.into_iter().enumerate() {
        match paint(n, slice) {
            Paint::None => {}
            Paint::Solid(color) => {
                let shape = shape_key(n, i);
                let _ = write!(out,r#"<use href="#{shape}" color="{}" fill-opacity="{}"/>"#,
                    color_to_rgb(color),color.a);
            }
            Paint::Layers(layers, owner) => {
                let mask = mask_key(n, i);
                let _ = write!(
                    out,
                    r#"<g opacity="{}" style="isolation:isolate" mask="url(#{mask})">"#,
                    n.opacity
                );
                for (index, layer) in layers.iter().enumerate().rev() {
                    let isolated = render::emit_composite_start(out, 1.0, layer.blend_mode());
                    let key = paint_key(n, owner, index);
                    let attrs = if let SceneFillLayer::Solid { color, .. } = layer {
                        format!(
                            r#"fill="{}" fill-opacity="{}""#,
                            color_to_rgb(*color),
                            color.a
                        )
                    } else {
                        format!(r#"fill="url(#{key})""#)
                    };
                    rect(out, area, &attrs);
                    if isolated {
                        out.push_str("</g>");
                    }
                }
                out.push_str("</g>");
            }
        }
    }
    out.push_str("</g>");
    Ok(())
}
