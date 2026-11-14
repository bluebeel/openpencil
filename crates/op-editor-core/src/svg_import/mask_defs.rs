//! SVG clip/mask resources expressed through existing canonical sibling masks.

use super::*;
use crate::svg_path_data::SvgPathBounds;
use jian_ops_schema::node::MaskType;
use kurbo::Affine;

pub(super) fn is_definition(tag: &str) -> bool {
    matches!(
        tag,
        "defs" | "lineargradient" | "radialgradient" | "stop" | "clippath" | "mask" | "symbol"
    )
}

pub(super) fn apply_resources(
    resources: &SvgResources<'_>,
    el: &SvgTree,
    mut node: PenNode,
    space: PaintSpace,
    allocator: &mut dyn crate::IdAllocator,
    taken: &mut std::collections::HashSet<NodeId>,
) -> Result<PenNode, crate::IdAllocError> {
    for property in ["clip-path", "mask"] {
        let Some(raw) = extract_style_or_attr(&el.attrs, property) else {
            continue;
        };
        if raw.trim() == "none" {
            continue;
        }
        let Some(id) = local_reference(&raw) else {
            tracing::warn!(
                property,
                "SVG mask/clip requires a local fragment reference"
            );
            continue;
        };
        let scope = match resources.enter(id) {
            Ok(scope) => scope,
            Err(error) => {
                tracing::warn!(%error,"SVG mask/clip reference rejected");
                continue;
            }
        };
        let definition = &resources.entries[id];
        let expected = if property == "clip-path" {
            "clippath"
        } else {
            "mask"
        };
        if definition.node.tag != expected {
            tracing::warn!(
                id,
                property,
                "SVG mask/clip referenced the wrong resource kind"
            );
            continue;
        }
        let source = build_source(
            resources,
            definition,
            property == "clip-path",
            space,
            allocator,
            taken,
        )?;
        drop(scope);
        let source = match source {
            Ok(source) => source,
            Err(error) => {
                tracing::warn!(%error,"SVG mask/clip resource cannot be represented");
                continue;
            }
        };
        // A valid empty resource masks everything. Keep an explicitly empty
        // alpha source instead of silently displaying unmasked content.
        let source = if let Some(source) = source {
            source
        } else {
            let id = allocator.allocate(taken)?;
            let bounds = space.target;
            let data = format!(
                "M{} {} H{} V{} H{} Z",
                bounds.x,
                bounds.y,
                bounds.x + bounds.w.max(1.0),
                bounds.y + bounds.h.max(1.0),
                bounds.x
            );
            let Some(mut empty) = path_node_from_svg_d(
                id,
                "Empty SVG mask",
                &data,
                Affine::IDENTITY,
                Some(vec![solid_fill("#000000")]),
                None,
                Some(PathFillRule::Nonzero),
            ) else {
                continue;
            };
            // An opaque black luminance mask has explicit bounds and zero
            // coverage, including backends that prune empty mask subtrees.
            empty.base_mut().mask_type = Some(MaskType::Luminance);
            empty
        };
        let id = allocator.allocate(taken)?;
        // Canonical children are topmost-first. The bottom mask source applies
        // to front siblings; group isolation stops it leaking to other nodes.
        node = import_group(id, property, vec![node, source]);
    }
    Ok(node)
}

fn build_source(
    resources: &SvgResources<'_>,
    entry: &Definition<'_>,
    clip: bool,
    space: PaintSpace,
    allocator: &mut dyn crate::IdAllocator,
    taken: &mut std::collections::HashSet<NodeId>,
) -> Result<Result<Option<PenNode>, SvgResourceError>, crate::IdAllocError> {
    let node = entry.node;
    let units = resource_attr(
        node,
        if clip {
            "clipPathUnits"
        } else {
            "maskContentUnits"
        },
    )
    .unwrap_or("userSpaceOnUse");
    let basis = match basis(units, space.source) {
        Ok(basis) => basis,
        Err(error) => return Ok(Err(error)),
    };
    if units == "objectBoundingBox" && (space.source.w <= 0.0 || space.source.h <= 0.0) {
        return Ok(Ok(None));
    }
    let transform = match transform_list(&node.attrs) {
        Ok(transform) => transform,
        Err(_) => {
            return Ok(Err(SvgResourceError::Invalid {
                attribute: "mask transform",
            }))
        }
    };
    let matrix = space.matrix * basis * transform;
    let mut context = entry.context.clone();
    context.clip_mode = clip;
    if clip {
        context.fill_rule = resource_attr(node, "clip-rule")
            .and_then(parse_svg_fill_rule)
            .or(Some(PathFillRule::Nonzero));
    }
    let mut children = Vec::new();
    for child in &node.children {
        if let Some(child) = element_to_node_ctx(
            child,
            &context,
            matrix,
            space.viewport,
            resources,
            allocator,
            taken,
        )? {
            children.push(child);
        }
    }
    children.reverse();
    if children.is_empty() {
        return Ok(Ok(None));
    }
    let mut source = if clip && children.len() == 1 {
        children.pop().expect("one clip child")
    } else {
        let id = allocator.allocate(taken)?;
        import_group(id, node.attr("id").unwrap_or("SVG mask"), children)
    };
    if !clip {
        source.base_mut().opacity = svg_node_opacity(&node.attrs);
        let region = match mask_region(node, space, transform) {
            Ok(region) => region,
            Err(error) => return Ok(Err(error)),
        };
        let id = allocator.allocate(taken)?;
        let Some(mut region) = path_node_from_svg_d(
            id,
            "SVG mask region",
            &region.0,
            region.1,
            Some(vec![solid_fill("#ffffff")]),
            None,
            Some(PathFillRule::Nonzero),
        ) else {
            return Ok(Ok(None));
        };
        region.base_mut().mask_type = Some(MaskType::Alpha);
        let id = allocator.allocate(taken)?;
        source = import_group(id, "SVG bounded mask", vec![source, region]);
    }
    let clip_is_primitive = matches!(
        &source,
        PenNode::Path(_) | PenNode::Rectangle(_) | PenNode::Ellipse(_)
    );
    source.base_mut().mask_type = Some(if clip {
        if clip_is_primitive {
            MaskType::Vector
        } else {
            MaskType::Alpha
        }
    } else {
        match extract_style_or_attr(&node.attrs, "mask-type")
            .as_deref()
            .unwrap_or("luminance")
        {
            "alpha" => MaskType::Alpha,
            "luminance" => {
                if entry.style.interpolation
                    == jian_ops_schema::style::GradientInterpolation::LinearRgb
                {
                    MaskType::LuminanceLinear
                } else {
                    MaskType::Luminance
                }
            }
            _ => {
                return Ok(Err(SvgResourceError::Invalid {
                    attribute: "mask-type",
                }))
            }
        }
    });
    Ok(Ok(Some(source)))
}
fn basis(units: &str, bounds: SvgPathBounds) -> Result<Affine, SvgResourceError> {
    match units {
        "userSpaceOnUse" => Ok(Affine::IDENTITY),
        "objectBoundingBox" => {
            Ok(Affine::translate((bounds.x, bounds.y))
                * Affine::scale_non_uniform(bounds.w, bounds.h))
        }
        _ => Err(SvgResourceError::Invalid {
            attribute: "mask coordinate units",
        }),
    }
}
fn mask_region(
    node: &SvgTree,
    space: PaintSpace,
    transform: Affine,
) -> Result<(String, Affine), SvgResourceError> {
    let units = resource_attr(node, "maskUnits").unwrap_or("objectBoundingBox");
    let basis = basis(units, space.source)?;
    let number = |name: &'static str, default: &str, axis: usize, position: bool| {
        let raw = resource_attr(node, name).unwrap_or(default).trim();
        let (raw, percent) = if let Some(raw) = raw.strip_suffix('%') {
            (raw, true)
        } else {
            (raw, false)
        };
        let value = raw
            .trim_end_matches("px")
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or(SvgResourceError::Invalid { attribute: name })?;
        Ok::<f64, SvgResourceError>(if !percent {
            value
        } else if units == "objectBoundingBox" {
            value * 0.01
        } else {
            value * 0.01 * space.viewport[axis + 2]
                + if position { space.viewport[axis] } else { 0.0 }
        })
    };
    let (x, y, w, h) = (
        number("x", "-10%", 0, true)?,
        number("y", "-10%", 1, true)?,
        number("width", "120%", 0, false)?,
        number("height", "120%", 1, false)?,
    );
    if w <= 0.0 || h <= 0.0 {
        return Ok(("M0 0".into(), Affine::IDENTITY));
    }
    Ok((
        format!("M{x} {y} H{} V{} H{x} Z", x + w, y + h),
        space.matrix * basis * transform,
    ))
}

pub(super) fn reference_bounds(el: &SvgTree) -> Option<SvgPathBounds> {
    fn visit(el: &SvgTree, matrix: Affine, apply: bool, out: &mut Option<SvgPathBounds>) {
        if is_definition(&el.tag) {
            return;
        }
        if extract_style_or_attr(&el.attrs, "display").as_deref() == Some("none") {
            return;
        }
        let matrix = if apply {
            match element_transform(el, matrix) {
                Ok(matrix) => matrix,
                Err(_) => return,
            }
        } else {
            matrix
        };
        if el.tag == "g" || el.tag == "svg" {
            for child in &el.children {
                visit(child, matrix, true, out);
            }
            return;
        }
        let Some(data) = shape_data(el) else {
            return;
        };
        let Some(bounds) = crate::svg_path_data::transformed_bounds(&data, matrix) else {
            return;
        };
        *out = Some(match *out {
            None => bounds,
            Some(old) => {
                let x = old.x.min(bounds.x);
                let y = old.y.min(bounds.y);
                SvgPathBounds {
                    x,
                    y,
                    w: (old.x + old.w).max(bounds.x + bounds.w) - x,
                    h: (old.y + old.h).max(bounds.y + bounds.h) - y,
                }
            }
        });
    }
    let mut out = None;
    visit(el, Affine::IDENTITY, false, &mut out);
    out
}
