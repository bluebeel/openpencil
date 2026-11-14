//! Affine-aware basic SVG shapes and scalar-stroke representation boundary.

use super::*;
use kurbo::Affine;
use std::{borrow::Cow, fmt::Write};

pub(super) fn transformed_leaf(
    el: &SvgTree,
    ctx: &StyleCtx,
    matrix: Affine,
    viewport: [f64; 4],
    resources: &SvgResources<'_>,
    allocator: &mut dyn crate::IdAllocator,
    taken: &mut std::collections::HashSet<NodeId>,
) -> Result<Option<PenNode>, crate::IdAllocError> {
    let Some(data) = shape_data(el) else {
        return Ok(None);
    };
    let Some(local_geometry) = crate::svg_path_data::localize_transformed(&data, matrix) else {
        return Ok(None);
    };
    let uses_server = ctx
        .fill
        .as_deref()
        .is_some_and(|raw| raw.trim().starts_with("url("))
        || ctx
            .stroke
            .as_deref()
            .is_some_and(|raw| raw.trim().starts_with("url("));
    let source_bounds = if uses_server {
        let Some(bounds) = crate::svg_path_data::svg_path_bounds(&data) else {
            return Ok(None);
        };
        bounds
    } else {
        local_geometry.1
    };
    let space = PaintSpace {
        source: source_bounds,
        target: local_geometry.1,
        matrix,
        viewport,
    };
    let fill = if ctx.clip_mode {
        Some(vec![solid_fill("#ffffff")])
    } else {
        resolve_paint(
            resources,
            ctx.fill.as_deref().unwrap_or("black"),
            ctx.fill_opacity,
            &ctx.current_color,
            space,
        )
    };
    let stroke_raw = if ctx.clip_mode {
        "none"
    } else {
        ctx.stroke.as_deref().unwrap_or("none")
    };
    let mut geometry_ctx = ctx.clone();
    geometry_ctx.stroke = Some(
        if stroke_raw.starts_with("url(") {
            "black"
        } else {
            stroke_raw
        }
        .to_string(),
    );
    let non_scaling =
        extract_style_or_attr(&el.attrs, "vector-effect").as_deref() == Some("non-scaling-stroke");
    let scalar = if non_scaling {
        Some(1.0)
    } else {
        similarity_scale(matrix)
    };
    let source_stroke = resolve_svg_stroke(&[], &geometry_ctx, 1.0);
    // A scalar canonical stroke cannot represent anisotropy/shear or a custom
    // miter limit. Expand it in the correct coordinate space before baking.
    let needs_outline = source_stroke.is_some()
        && (scalar.is_none() || ctx.stroke_miter != 4.0 || stroke_raw.starts_with("url("));
    let id = allocator.allocate(taken)?;
    let name = el.attr("id").unwrap_or(match el.tag.as_str() {
        "rect" => "Rect",
        "circle" | "ellipse" => "Ellipse",
        "line" => "Line",
        _ => "Path",
    });
    if !needs_outline {
        let mut stroke = resolve_svg_stroke(&[], &geometry_ctx, scalar.unwrap_or(1.0));
        if source_stroke.is_some() && stroke.is_none() {
            tracing::warn!(tag = %el.tag, "SVG transformed stroke is outside finite scalar range");
            return Ok(None);
        }
        if let Some(stroke) = &mut stroke {
            stroke.fill = resolve_paint(
                resources,
                stroke_raw,
                ctx.stroke_opacity,
                &ctx.current_color,
                space,
            );
        }
        if stroke.as_ref().is_some_and(|stroke| stroke.fill.is_none()) {
            stroke = None;
        }
        if let Some(mut node) = typed_axis_shape(el, id.clone(), matrix) {
            node.base_mut().name = Some(name.to_string());
            clear_node_fill(&mut node);
            if let Some(fill) = fill {
                set_fill(&mut node, fill);
            }
            if let Some(stroke) = stroke {
                set_node_stroke(&mut node, stroke);
            }
            return Ok(Some(node));
        }
        return Ok(path_node_from_local_geometry(
            id,
            name,
            local_geometry,
            fill,
            stroke,
            ctx.fill_rule,
        ));
    }
    let Some(mut source) = crate::svg_path_data::cubic_path(&data) else {
        tracing::warn!(tag = %el.tag, "SVG transformed stroke rejected invalid path geometry");
        return Ok(None);
    };
    if non_scaling {
        source.apply_affine(matrix);
        if !source.is_finite() {
            tracing::warn!(tag = %el.tag, "SVG non-scaling stroke overflowed transformed geometry");
            return Ok(None);
        }
    }
    let style = kurbo::Stroke::new(ctx.stroke_width)
        .with_caps(ctx.stroke_cap)
        .with_join(ctx.stroke_join)
        .with_miter_limit(ctx.stroke_miter)
        .with_dashes(ctx.stroke_dash_offset, &ctx.stroke_dashes);
    let tolerance = (ctx.stroke_width.max(1.0) * 1e-7).max(1e-7);
    let mut outline = kurbo::stroke(
        source.elements().iter().copied(),
        &style,
        &kurbo::StrokeOpts::default(),
        tolerance,
    );
    if !non_scaling {
        outline.apply_affine(matrix);
    }
    if !outline.is_finite() {
        tracing::warn!(tag = %el.tag, "SVG transformed stroke overflowed finite geometry");
        return Ok(None);
    }
    let mut children = Vec::with_capacity(2);
    if let Some(fill) = fill {
        if let Some(node) = path_node_from_local_geometry(
            id.clone(),
            name,
            local_geometry,
            Some(fill),
            None,
            ctx.fill_rule,
        ) {
            children.push(node);
        }
    }
    if !outline.elements().is_empty() {
        let outline_id = if children.is_empty() {
            id.clone()
        } else {
            allocator.allocate(taken)?
        };
        let outline_data = outline.to_svg();
        let Some(outline_local) =
            crate::svg_path_data::localize_transformed(&outline_data, Affine::IDENTITY)
        else {
            return Ok(None);
        };
        let paint = resolve_paint(
            resources,
            stroke_raw,
            ctx.stroke_opacity,
            &ctx.current_color,
            PaintSpace {
                target: outline_local.1,
                ..space
            },
        );
        if paint.is_some() {
            if let Some(node) = path_node_from_local_geometry(
                outline_id,
                name,
                outline_local,
                paint,
                None,
                Some(PathFillRule::Nonzero),
            ) {
                children.push(node);
            }
        }
    }
    if children.is_empty() {
        return Ok(None);
    }
    if children.len() == 1 {
        return Ok(children.pop());
    }
    let group = allocator.allocate(taken)?;
    Ok(Some(import_group(group, name, children)))
}

fn set_fill(node: &mut PenNode, fill: Vec<PenFill>) {
    match node {
        PenNode::Rectangle(node) => node.container.fill = Some(fill),
        PenNode::Ellipse(node) => node.fill = Some(fill),
        _ => unreachable!("only axis-aligned basic typed shapes enter this helper"),
    }
}

fn typed_axis_shape(el: &SvgTree, id: NodeId, matrix: Affine) -> Option<PenNode> {
    let [a, b, c, d, tx, ty] = matrix.as_coeffs();
    if b != 0.0 || c != 0.0 || a == 0.0 || d == 0.0 {
        return None;
    }
    let (x, y, w, h) = match el.tag.as_str() {
        "rect" if el.num("rx") == 0.0 && el.num("ry") == 0.0 => {
            (el.num("x"), el.num("y"), el.num("width"), el.num("height"))
        }
        "circle" => {
            let r = el.num("r");
            (el.num("cx") - r, el.num("cy") - r, r * 2.0, r * 2.0)
        }
        "ellipse" => {
            let (rx, ry) = (el.num("rx"), el.num("ry"));
            (el.num("cx") - rx, el.num("cy") - ry, rx * 2.0, ry * 2.0)
        }
        _ => return None,
    };
    let (left, right, top, bottom) = (a * x + tx, a * (x + w) + tx, d * y + ty, d * (y + h) + ty);
    if ![left, right, top, bottom, right - left, bottom - top]
        .iter()
        .all(|v| v.is_finite())
    {
        return None;
    }
    // Reuse canonical node defaults; overwrite integer legacy construction with
    // exact transformed f64 placement and sizing. Do not quantize SVG geometry.
    let mut legacy = SvgElement {
        tag: el.tag.clone(),
        attrs: el.attrs.clone(),
    };
    apply_scale_to_attrs(&mut legacy, a.abs());
    let mut node = element_to_node(&legacy, id, (0.0, 0.0))?;
    node.base_mut().x = Some(left.min(right));
    node.base_mut().y = Some(top.min(bottom));
    let (width, height) = (
        Some(SizingBehavior::Number((right - left).abs())),
        Some(SizingBehavior::Number((bottom - top).abs())),
    );
    match &mut node {
        PenNode::Rectangle(node) => {
            node.container.width = width;
            node.container.height = height;
        }
        PenNode::Ellipse(node) => {
            node.width = width;
            node.height = height;
        }
        _ => return None,
    }
    Some(node)
}

pub(super) fn shape_data(el: &SvgTree) -> Option<Cow<'_, str>> {
    let mut d = String::new();
    match el.tag.as_str() {
        "path" => return el.attr("d").map(Cow::Borrowed),
        "rect" => {
            let (x, y, w, h) = (el.num("x"), el.num("y"), el.num("width"), el.num("height"));
            if ![x, y, w, h].iter().all(|v| v.is_finite()) || w <= 0.0 || h <= 0.0 {
                return None;
            }
            let rx = if el.attr("rx").is_some() {
                el.num("rx")
            } else {
                el.num("ry")
            };
            let ry = if el.attr("ry").is_some() {
                el.num("ry")
            } else {
                rx
            };
            if !rx.is_finite() || !ry.is_finite() || rx < 0.0 || ry < 0.0 {
                return None;
            }
            let (rx, ry) = (rx.min(w * 0.5), ry.min(h * 0.5));
            if rx == 0.0 || ry == 0.0 {
                write!(d, "M{x} {y} H{} V{} H{x} Z", x + w, y + h).ok()?;
            } else {
                write!(d, "M{} {y} H{} A{rx} {ry} 0 0 1 {} {} V{} A{rx} {ry} 0 0 1 {} {} H{} A{rx} {ry} 0 0 1 {x} {} V{} A{rx} {ry} 0 0 1 {} {y} Z",
                    x + rx, x + w - rx, x + w, y + ry, y + h - ry, x + w - rx, y + h,
                    x + rx, y + h - ry, y + ry, x + rx).ok()?;
            }
        }
        "circle" | "ellipse" => {
            let (x, y) = (el.num("cx"), el.num("cy"));
            let (rx, ry) = if el.tag == "circle" {
                (el.num("r"), el.num("r"))
            } else {
                (el.num("rx"), el.num("ry"))
            };
            if ![x, y, rx, ry].iter().all(|v| v.is_finite()) || rx <= 0.0 || ry <= 0.0 {
                return None;
            }
            write!(
                d,
                "M{} {y} A{rx} {ry} 0 1 1 {} {y} A{rx} {ry} 0 1 1 {} {y} Z",
                x + rx,
                x - rx,
                x + rx
            )
            .ok()?;
        }
        "line" => {
            write!(
                d,
                "M{} {} L{} {}",
                el.num("x1"),
                el.num("y1"),
                el.num("x2"),
                el.num("y2")
            )
            .ok()?;
        }
        "polyline" | "polygon" => {
            let points = parse_point_list(el.attr("points")?);
            if points.len() < 2 {
                return None;
            }
            for (index, (x, y)) in points.into_iter().enumerate() {
                write!(d, "{}{} {} ", if index == 0 { 'M' } else { 'L' }, x, y).ok()?;
            }
            if el.tag == "polygon" {
                d.push('Z');
            }
        }
        _ => return None,
    }
    Some(Cow::Owned(d))
}
