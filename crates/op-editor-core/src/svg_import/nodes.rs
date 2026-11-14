//! Element -> `PenNode` conversion: the style-aware recursive builder
//! plus the per-shape leaf/path constructors.

use super::*;

/// Build a `PenNode` for an SVG element with inherited style
/// context + viewBox scaling applied. `<g>` recurses into its
/// children and wraps them in a `PenNode::Group`.
pub(super) fn element_to_node_ctx(
    el: &SvgTree,
    parent_ctx: &StyleCtx,
    parent_matrix: kurbo::Affine,
    parent_viewport: [f64; 4],
    resources: &SvgResources<'_>,
    allocator: &mut dyn crate::IdAllocator,
    taken: &mut std::collections::HashSet<NodeId>,
) -> Result<Option<PenNode>, crate::IdAllocError> {
    if is_definition(&el.tag)
        || extract_style_or_attr(&el.attrs, "display").as_deref() == Some("none")
    {
        return Ok(None);
    }
    let ctx = merge_style_ctx(parent_ctx, &el.attrs);
    let matrix = match element_transform(el, parent_matrix) {
        Ok(matrix) => matrix,
        Err(error) => {
            tracing::warn!(%error,tag=%el.tag,"SVG element transform import rejected");
            return Ok(None);
        }
    };
    let viewport = if el.tag == "svg" {
        match source_viewport(&el.attrs, parent_viewport) {
            Ok(viewport) => viewport,
            Err(error) => {
                tracing::warn!(%error,"SVG nested viewport import rejected");
                return Ok(None);
            }
        }
    } else {
        parent_viewport
    };
    let mut node = if el.tag == "g" || el.tag == "svg" {
        if el.tag == "svg" {
            tracing::warn!("nested SVG viewport mapping is imported; automatic viewport clipping is not represented");
        }
        let mut children = Vec::new();
        for child in &el.children {
            if let Some(child) =
                element_to_node_ctx(child, &ctx, matrix, viewport, resources, allocator, taken)?
            {
                children.push(child);
            }
        }
        if children.is_empty() {
            return Ok(None);
        }
        children.reverse();
        let explicit_group = el.attr("id").is_some()
            || svg_node_opacity(&el.attrs).is_some()
            || ["clip-path", "mask", "mix-blend-mode"]
                .iter()
                .any(|name| extract_style_or_attr(&el.attrs, name).is_some());
        if children.len() == 1 && !explicit_group {
            children.pop().expect("one SVG child")
        } else {
            let id = allocator.allocate(taken)?;
            import_group(id, el.attr("id").unwrap_or("Group"), children)
        }
    } else {
        let Some(node) = transformed_leaf(el, &ctx, matrix, viewport, resources, allocator, taken)?
        else {
            return Ok(None);
        };
        node
    };
    let has_resource = ["clip-path", "mask"].iter().any(|property| {
        extract_style_or_attr(&el.attrs, property).is_some_and(|value| value.trim() != "none")
    });
    if has_resource {
        if let Some(source) = reference_bounds(el) {
            let target = crate::svg_path_data::transformed_bounds(
                &shape_data(el).unwrap_or_else(|| {
                    std::borrow::Cow::Owned(format!(
                        "M{} {} H{} V{} H{} Z",
                        source.x,
                        source.y,
                        source.x + source.w,
                        source.y + source.h,
                        source.x
                    ))
                }),
                matrix,
            )
            .unwrap_or(source);
            node = apply_resources(
                resources,
                el,
                node,
                PaintSpace {
                    source,
                    target,
                    matrix,
                    viewport,
                },
                allocator,
                taken,
            )?;
        }
    }
    if !ctx.clip_mode {
        node.base_mut().opacity = svg_node_opacity(&el.attrs);
        if let Some(mode) = extract_style_or_attr(&el.attrs, "mix-blend-mode") {
            node.base_mut().blend_mode =
                serde_json::from_value(serde_json::Value::String(mode.replace('-', '_'))).ok();
        }
    }
    if extract_style_or_attr(&el.attrs, "visibility").as_deref() == Some("hidden") {
        node.base_mut().visible = Some(false);
    }
    Ok(Some(node))
}

pub(super) fn import_group(id: NodeId, name: &str, children: Vec<PenNode>) -> PenNode {
    use jian_ops_schema::node::{container::ContainerProps, GroupNode};
    PenNode::Group(GroupNode {
        base: PenNodeBase {
            id: id.into(),
            name: Some(name.to_string()),
            ..Default::default()
        },
        container: ContainerProps::default(),
        children: Some(children),
        state: None,
        bindings: None,
        events: None,
        lifecycle: None,
        semantics: None,
        gestures: None,
        route: None,
    })
}

/// Build a `PenNode` from one parsed SVG element. `None` for
/// unsupported tags (`g` / `svg` / `defs` / `style` / …) and for
/// degenerate geometry.
pub(super) fn element_to_node(el: &SvgElement, id: NodeId, offset: (f64, f64)) -> Option<PenNode> {
    let (ox, oy) = offset;
    let fill = el.attr("fill").and_then(parse_svg_color);
    let mut node = match el.tag.as_str() {
        "rect" => {
            let (w, h) = (el.num("width"), el.num("height"));
            if w <= 0.0 || h <= 0.0 {
                return None;
            }
            build_leaf_node(
                "rect",
                id.as_str(),
                "Rect",
                (el.num("x") + ox).round() as i32,
                (el.num("y") + oy).round() as i32,
                w.round() as i32,
                h.round() as i32,
            )?
        }
        "circle" => {
            let r = el.num("r");
            if r <= 0.0 {
                return None;
            }
            build_leaf_node(
                "ellipse",
                id.as_str(),
                "Ellipse",
                (el.num("cx") - r + ox).round() as i32,
                (el.num("cy") - r + oy).round() as i32,
                (r * 2.0).round() as i32,
                (r * 2.0).round() as i32,
            )?
        }
        "ellipse" => {
            let (rx, ry) = (el.num("rx"), el.num("ry"));
            if rx <= 0.0 || ry <= 0.0 {
                return None;
            }
            build_leaf_node(
                "ellipse",
                id.as_str(),
                "Ellipse",
                (el.num("cx") - rx + ox).round() as i32,
                (el.num("cy") - ry + oy).round() as i32,
                (rx * 2.0).round() as i32,
                (ry * 2.0).round() as i32,
            )?
        }
        "line" => {
            let p0 = (el.num("x1") + ox, el.num("y1") + oy);
            let p1 = (el.num("x2") + ox, el.num("y2") + oy);
            path_node_from_anchors(id, "Line", &[p0, p1], false)?
        }
        "polyline" | "polygon" => {
            let pts: Vec<(f64, f64)> = parse_point_list(el.attr("points").unwrap_or(""))
                .into_iter()
                .map(|(x, y)| (x + ox, y + oy))
                .collect();
            if pts.len() < 2 {
                return None;
            }
            path_node_from_anchors(id, "Path", &pts, el.tag == "polygon")?
        }
        "path" => {
            // The style-aware importer preserves compound d as one node.
            // This legacy single-contour builder retains its editable-anchor
            // contract and intentionally returns only the first contour.
            let d = el.attr("d")?;
            let (anchors, closed) = parse_path_d(d, offset).into_iter().next()?;
            if anchors.len() < 2 {
                return None;
            }
            path_node_from_pen_anchors(id, anchors, closed)?
        }
        // <svg> / <g> / <defs> / <style> / <title> / … — skipped.
        _ => return None,
    };
    if let Some(hex) = fill {
        set_primary_fill_hex(&mut node, &hex);
    }
    Some(node)
}

/// Build a straight-segment `Path` node from doc-space points.
fn path_node_from_anchors(
    id: NodeId,
    name: &str,
    pts: &[(f64, f64)],
    closed: bool,
) -> Option<PenNode> {
    let anchors: Vec<PenPathAnchor> = pts
        .iter()
        .map(|&(x, y)| PenPathAnchor {
            x,
            y,
            handle_in: None,
            handle_out: None,
            point_type: None,
        })
        .collect();
    path_node_from_pen_anchors(id, anchors, closed).map(|mut n| {
        n.base_mut().name = Some(name.to_string());
        n
    })
}

/// Build a `Path` node from ready `PenPathAnchor`s, fitting the base
/// rect to the anchor bounding box.
fn path_node_from_pen_anchors(
    id: NodeId,
    anchors: Vec<PenPathAnchor>,
    closed: bool,
) -> Option<PenNode> {
    if anchors.len() < 2 {
        return None;
    }
    let (min_x, min_y, max_x, max_y) = crate::svg_path_bounds::path_anchor_bounds(&anchors, closed);
    Some(PenNode::Path(PathNode {
        base: PenNodeBase {
            id: id.into(),
            name: Some("Path".to_string()),
            x: Some(min_x),
            y: Some(min_y),
            ..Default::default()
        },
        icon_id: None,
        d: None,
        anchors: Some(anchors),
        closed: Some(closed),
        fill_rule: None,
        mask: None,
        width: Some(SizingBehavior::Number((max_x - min_x).max(0.0))),
        height: Some(SizingBehavior::Number((max_y - min_y).max(0.0))),
        fill: None,
        stroke: None,
        effects: None,
        state: None,
        bindings: None,
        events: None,
        lifecycle: None,
        semantics: None,
        gestures: None,
        route: None,
        limits: Default::default(),
    }))
}

/// Localize already-transformed f64 SVG geometry without rounding vertices.
pub(super) fn path_node_from_svg_d(
    id: NodeId,
    name: &str,
    d: &str,
    matrix: kurbo::Affine,
    fill: Option<Vec<PenFill>>,
    stroke: Option<PenStroke>,
    fill_rule: Option<PathFillRule>,
) -> Option<PenNode> {
    let Some((local_d, bounds)) = crate::svg_path_data::localize_transformed(d, matrix) else {
        tracing::warn!("SVG path transform could not be represented as finite geometry");
        return None;
    };
    path_node_from_local_geometry(id, name, (local_d, bounds), fill, stroke, fill_rule)
}

pub(super) fn path_node_from_local_geometry(
    id: NodeId,
    name: &str,
    geometry: (String, crate::svg_path_data::SvgPathBounds),
    fill: Option<Vec<PenFill>>,
    stroke: Option<PenStroke>,
    fill_rule: Option<PathFillRule>,
) -> Option<PenNode> {
    let (local_d, bounds) = geometry;
    let closed = local_d.contains('Z');
    Some(PenNode::Path(PathNode {
        base: PenNodeBase {
            id: id.into(),
            name: Some(name.to_string()),
            x: Some(bounds.x),
            y: Some(bounds.y),
            ..Default::default()
        },
        icon_id: None,
        d: Some(local_d),
        anchors: None,
        closed: Some(closed),
        fill_rule,
        mask: None,
        width: Some(SizingBehavior::Number(bounds.w)),
        height: Some(SizingBehavior::Number(bounds.h)),
        fill,
        stroke,
        effects: None,
        state: None,
        bindings: None,
        events: None,
        lifecycle: None,
        semantics: None,
        gestures: None,
        route: None,
        limits: Default::default(),
    }))
}
