//! Figma node converters — ports `converters/index.ts` and the
//! per-type converter modules. Walks the [`TreeNode`] tree and emits
//! canonical `PenNode`s.

use crate::boolean_fallback::{convert_empty_boolean_group, convert_swapped_boolean_group};
use crate::common::{
    common_props, lookup_icon_by_name, map_corner_radius, map_corner_smoothing, normalize_angle,
    resolve_height, resolve_width, round2, round3, ConversionContext, FigLayoutMode, IconStyle,
    SKIPPED_TYPES,
};
use crate::corner_geometry::smoothed_rect_path;
use crate::figma_types::FigVec2;
use crate::instance::{apply_instance_overrides_shared, merge_symbol_props};
use crate::kiwi::FigValue;
use crate::layout_positioning::{
    apply_layout_positioning, order_children, sizing_parent_stack_mode,
};
use crate::mappers::{
    fig_fill_color, map_figma_fills, map_figma_layout, map_figma_node_effects, map_figma_stroke,
    LayoutProps,
};
use crate::mask::{any_visible, figma_path_mask};
use crate::node_build::{
    ellipse_node, frame_node, group_node, line_node, path_node, rectangle_node, ref_node,
};
use crate::tree::{guid_to_string, TreeNode};
use crate::vector_decoder::{compute_svg_path_bounds, decode_figma_vector_path, DecodedVectorPath};
use crate::vector_fallback::decode_single_child_boolean;
use jian_ops_schema::node::base::NumberOrExpression;
use jian_ops_schema::node::container::ContainerProps;
use jian_ops_schema::node::PenNode;
use jian_ops_schema::sizing::SizingBehavior;

mod text;
use text::convert_text;

/// Spread mapped [`LayoutProps`] onto a `ContainerProps`.
fn apply_layout(l: LayoutProps, c: &mut ContainerProps) {
    if l.layout.is_some() {
        c.layout = l.layout;
    }
    if let Some(g) = l.gap {
        c.gap = Some(NumberOrExpression::Number(g));
    }
    if l.padding.is_some() {
        c.padding = l.padding;
    }
    if l.justify_content.is_some() {
        c.justify_content = l.justify_content;
    }
    if l.align_items.is_some() {
        c.align_items = l.align_items;
    }
    if l.clip_content.is_some() {
        c.clip_content = l.clip_content;
    }
}

/// Convert all authored children; visibility/opacity control painting, not existence.
pub fn convert_children(parent: &TreeNode, ctx: &mut ConversionContext<'_>) -> Vec<PenNode> {
    convert_children_parts(&parent.figma, &parent.children, ctx)
}
fn convert_children_parts(
    figma: &FigValue,
    children: &[TreeNode],
    ctx: &mut ConversionContext<'_>,
) -> Vec<PenNode> {
    let parent_stack_mode = figma.get_str("stackMode");
    let mut result = Vec::new();
    for child in children {
        if let Some(node) = convert_retained_child(child, parent_stack_mode, ctx) {
            result.push(node);
        }
    }
    result
}

/// This entry replaces the historical child filter, so only formerly omitted
/// branches use new IDs. Direct selected-root conversion keeps its old policy.
pub(crate) fn convert_retained_child(
    tree: &TreeNode,
    parent: Option<&str>,
    ctx: &mut ConversionContext<'_>,
) -> Option<PenNode> {
    let omitted = tree.figma.get_bool("visible") == Some(false)
        || tree
            .figma
            .get_f64("opacity")
            .is_some_and(|opacity| opacity <= 0.0);
    ctx.retain_hidden_ids(omitted, |ctx| convert_node(tree, parent, ctx))
}

/// Dispatch one tree node to its type-specific converter.
pub fn convert_node(
    tree: &TreeNode,
    parent_stack_mode: Option<&str>,
    ctx: &mut ConversionContext<'_>,
) -> Option<PenNode> {
    let ty = tree.figma.get_str("type")?;
    if SKIPPED_TYPES.contains(&ty) {
        return None;
    }
    let sizing_parent = sizing_parent_stack_mode(&tree.figma, parent_stack_mode, ctx.layout_mode);
    let mut node = match ty {
        "FRAME" | "SECTION" => convert_frame(tree, sizing_parent, ctx),
        "GROUP" => convert_group(tree, sizing_parent, ctx),
        "SYMBOL" => convert_component(tree, sizing_parent, ctx),
        "INSTANCE" => convert_instance(tree, sizing_parent, ctx),
        "RECTANGLE" | "ROUNDED_RECTANGLE" => convert_rectangle(tree, sizing_parent, ctx),
        "ELLIPSE" => convert_ellipse(tree, sizing_parent, ctx),
        "LINE" => convert_line(tree, ctx),
        "VECTOR" | "STAR" | "REGULAR_POLYGON" | "BOOLEAN_OPERATION" => {
            convert_vector(tree, sizing_parent, ctx)
        }
        "TEXT" => convert_text(tree, sizing_parent, ctx),
        _ => {
            if !tree.children.is_empty() {
                convert_frame(tree, sizing_parent, ctx)
            } else {
                ctx.warnings.push(format!(
                    "Skipped unsupported node type: {ty} ({})",
                    tree.figma.get_str("name").unwrap_or("")
                ));
                return None;
            }
        }
    };
    apply_layout_positioning(&mut node, &tree.figma, parent_stack_mode, ctx.layout_mode);
    Some(node)
}

/// Build the `{ width, height, layout?, cornerRadius, fill, stroke,
/// effects }` container shared by frame / component.
fn build_container(
    figma: &FigValue,
    parent_stack_mode: Option<&str>,
    has_auto_layout: bool,
    ctx: &ConversionContext<'_>,
) -> ContainerProps {
    let mut container = ContainerProps {
        width: Some(resolve_width(figma, parent_stack_mode, ctx)),
        height: Some(resolve_height(figma, parent_stack_mode, ctx)),
        ..ContainerProps::default()
    };
    let layout: Option<LayoutProps> = match ctx.layout_mode {
        FigLayoutMode::Preserve => {
            if has_auto_layout {
                Some(map_figma_layout(figma))
            } else {
                Some(LayoutProps {
                    // Preserve the inverse Figma flag explicitly. An absent
                    // canonical value is reserved for legacy root-frame
                    // clipping compatibility in the loader.
                    clip_content: Some(figma.get_bool("frameMaskDisabled") != Some(true)),
                    ..LayoutProps::default()
                })
            }
        }
        FigLayoutMode::OpenPencil => Some(map_figma_layout(figma)),
    };
    if let Some(l) = layout {
        apply_layout(l, &mut container);
    }
    container.corner_radius = map_corner_radius(figma);
    let fills = figma.get_array("fillPaints");
    container.fill = if fills.is_some_and(|fills| !fills.is_empty()) {
        map_figma_fills(fills)
    } else {
        // Legacy background paints predate fillPaints. Preserve that active
        // fallback only for an absent/empty new stack, never a disabled one.
        map_figma_fills(figma.get_array("backgroundPaints"))
            .filter(|fills| !fills.is_empty())
            .or_else(|| map_figma_fills(fills))
    };
    container.stroke = map_figma_stroke(figma);
    container.effects = map_figma_node_effects(figma);
    container
}

fn convert_frame(
    tree: &TreeNode,
    parent_stack_mode: Option<&str>,
    ctx: &mut ConversionContext<'_>,
) -> PenNode {
    convert_frame_parts(&tree.figma, &tree.children, parent_stack_mode, ctx)
}
fn convert_frame_parts(
    figma: &FigValue,
    children: &[TreeNode],
    parent_stack_mode: Option<&str>,
    ctx: &mut ConversionContext<'_>,
) -> PenNode {
    convert_frame_parts_with_retained_ids(figma, children, parent_stack_mode, ctx, false)
}
fn convert_frame_parts_with_retained_ids(
    figma: &FigValue,
    children: &[TreeNode],
    parent_stack_mode: Option<&str>,
    ctx: &mut ConversionContext<'_>,
    retained: bool,
) -> PenNode {
    let id = ctx.generate_id();
    let children =
        ctx.retain_hidden_ids(retained, |ctx| convert_children_parts(figma, children, ctx));
    let has_auto_layout = figma
        .get_str("stackMode")
        .map(|s| s != "NONE")
        .unwrap_or(false);
    let container = build_container(figma, parent_stack_mode, has_auto_layout, ctx);
    let ordered = order_children(children, figma, ctx.layout_mode);
    let base = common_props(figma, id);
    frame_node(
        base,
        container,
        if ordered.is_empty() {
            None
        } else {
            Some(ordered)
        },
        None,
    )
}

fn convert_component(
    tree: &TreeNode,
    parent_stack_mode: Option<&str>,
    ctx: &mut ConversionContext<'_>,
) -> PenNode {
    let figma_id = tree
        .figma
        .get("guid")
        .and_then(guid_to_string)
        .unwrap_or_default();
    let id = ctx
        .component_map
        .get(&figma_id)
        .cloned()
        .unwrap_or_else(|| ctx.generate_id());
    let children = convert_children(tree, ctx);
    let figma = &tree.figma;
    let has_auto_layout = figma
        .get_str("stackMode")
        .map(|s| s != "NONE")
        .unwrap_or(false);
    let container = build_container(figma, parent_stack_mode, has_auto_layout, ctx);
    let ordered = order_children(children, figma, ctx.layout_mode);
    let base = common_props(figma, id);
    frame_node(
        base,
        container,
        if ordered.is_empty() {
            None
        } else {
            Some(ordered)
        },
        Some(true),
    )
}

fn convert_group(
    tree: &TreeNode,
    parent_stack_mode: Option<&str>,
    ctx: &mut ConversionContext<'_>,
) -> PenNode {
    let id = ctx.generate_id();
    let children = convert_children(tree, ctx);
    let figma = &tree.figma;
    let container = ContainerProps {
        width: Some(resolve_width(figma, parent_stack_mode, ctx)),
        height: Some(resolve_height(figma, parent_stack_mode, ctx)),
        ..ContainerProps::default()
    };
    let base = common_props(figma, id);
    group_node(
        base,
        container,
        if children.is_empty() {
            None
        } else {
            Some(children)
        },
    )
}

/// Whether a symbol instance carries visual (non-layout) overrides.
fn has_visual_overrides(figma: &FigValue) -> bool {
    figma
        .get("symbolData")
        .and_then(|s| s.get_array("symbolOverrides"))
        .map(|ovs| {
            ovs.iter().any(|ov| {
                ov.get_array("fillPaints").map(|p| !p.is_empty()) == Some(true)
                    || ov.get_array("strokePaints").map(|p| !p.is_empty()) == Some(true)
                    || ov.get("arcData").is_some()
                    || ov.get("textData").is_some()
                    || ov.get("fontSize").is_some()
                    || ov.get("overriddenSymbolID").is_some()
                    || ov
                        .get_array("componentPropAssignments")
                        .is_some_and(|assignments| !assignments.is_empty())
            })
        })
        .unwrap_or(false)
}

/// Additional authored paint/visibility overrides that older imports omitted.
fn has_authoring_overrides(figma: &FigValue) -> bool {
    ["fillPaints", "strokePaints", "effects"]
        .iter()
        .any(|key| figma.get(key).is_some())
        || figma
            .get("symbolData")
            .and_then(|data| data.get_array("symbolOverrides"))
            .is_some_and(|overrides| {
                overrides.iter().any(|entry| {
                    [
                        "visible",
                        "opacity",
                        "fillPaints",
                        "strokePaints",
                        "effects",
                    ]
                    .iter()
                    .any(|key| entry.get(key).is_some())
                })
            })
}

fn convert_instance(
    tree: &TreeNode,
    parent_stack_mode: Option<&str>,
    ctx: &mut ConversionContext<'_>,
) -> PenNode {
    let figma = &tree.figma;
    let override_guid = figma.get("overriddenSymbolID").and_then(guid_to_string);
    let base_guid = figma
        .get("symbolData")
        .and_then(|data| data.get("symbolID"))
        .and_then(guid_to_string);
    let component_guid = override_guid.clone().or_else(|| base_guid.clone());

    // A real component swap must expand the target instead of stale children.
    let is_swap = override_guid
        .as_ref()
        .is_some_and(|target| base_guid.as_ref() != Some(target));
    let legacy_inline = is_swap || tree.children.is_empty() || has_visual_overrides(figma);
    let restored_inline = !legacy_inline && has_authoring_overrides(figma);
    let inline = component_guid.is_some() && (legacy_inline || restored_inline);

    if inline {
        let key = component_guid.clone().unwrap();
        // Copy the borrowed master reference out of the map before mutating
        // the rest of the conversion context during recursive expansion.
        if let Some(symbol_node) = ctx.symbol_tree.get(&key).copied() {
            if !symbol_node.children.is_empty() {
                let overrides = figma
                    .get("symbolData")
                    .and_then(|s| s.get_array("symbolOverrides"));
                let base_symbol_node = base_guid
                    .as_ref()
                    .and_then(|guid| ctx.symbol_tree.get(guid))
                    .copied();
                let instance_size = figma.get("size").and_then(FigVec2::from_value);
                // A component swap (`overriddenSymbolID` differs from the
                // base `symbolID`) leaves the pre-swap component's stale
                // derived cluster in the array — drop it so it can't
                // hijack the fingerprint mapping onto the swapped subtree.
                let derived = figma.get_array("derivedSymbolData").map(|a| {
                    if is_swap {
                        std::borrow::Cow::Owned(crate::instance::filter_swap_stale_derived(
                            a,
                            base_symbol_node,
                            symbol_node,
                            instance_size,
                        ))
                    } else {
                        std::borrow::Cow::Borrowed(a)
                    }
                });
                let children = apply_instance_overrides_shared(
                    &mut ctx.instance_expansions,
                    &key,
                    symbol_node,
                    overrides,
                    derived.as_deref(),
                    instance_size,
                    &mut ctx.instance_assignments,
                );
                let merged = merge_symbol_props(&tree.figma, &symbol_node.figma);
                return convert_frame_parts_with_retained_ids(
                    &merged,
                    children.as_slice(),
                    parent_stack_mode,
                    ctx,
                    restored_inline,
                );
            }
        }
    }

    // No inlining — emit a ref when the component is registered.
    if let Some(pen_id) = component_guid
        .as_ref()
        .and_then(|g| ctx.component_map.get(g))
        .cloned()
    {
        let id = ctx.generate_id();
        let mut base = common_props(&tree.figma, id);
        let symbol = component_guid
            .as_ref()
            .and_then(|guid| ctx.symbol_tree.get(guid))
            .map(|tree| &tree.figma);
        base.visible = Some(crate::instance::instance_visibility(&tree.figma, symbol));
        return ref_node(base, pen_id);
    }

    convert_frame(tree, parent_stack_mode, ctx)
}

fn convert_rectangle(
    tree: &TreeNode,
    parent_stack_mode: Option<&str>,
    ctx: &mut ConversionContext<'_>,
) -> PenNode {
    let id = ctx.generate_id();
    let figma = &tree.figma;
    let corner_radius = map_corner_radius(figma);
    let smoothing = map_corner_smoothing(figma);
    if smoothing > 0.0 {
        let size = figma.get("size");
        let width_px = size.and_then(|value| value.get_f64("x")).unwrap_or(0.0);
        let height_px = size.and_then(|value| value.get_f64("y")).unwrap_or(0.0);
        let radii = match corner_radius.as_ref() {
            Some(jian_ops_schema::node::container::CornerRadius::Uniform(radius)) => {
                Some([*radius; 4])
            }
            Some(jian_ops_schema::node::container::CornerRadius::PerCorner(radii)) => Some(*radii),
            None => None,
        };
        if let Some(path_d) =
            radii.and_then(|radii| smoothed_rect_path(width_px, height_px, radii, smoothing))
        {
            return path_node(
                common_props(figma, id),
                Some(path_d),
                None,
                None,
                resolve_width(figma, parent_stack_mode, ctx),
                resolve_height(figma, parent_stack_mode, ctx),
                map_figma_fills(figma.get_array("fillPaints")),
                map_figma_stroke(figma),
                map_figma_node_effects(figma),
                figma_path_mask(figma),
            );
        }
    }
    let container = ContainerProps {
        width: Some(resolve_width(figma, parent_stack_mode, ctx)),
        height: Some(resolve_height(figma, parent_stack_mode, ctx)),
        corner_radius,
        fill: map_figma_fills(figma.get_array("fillPaints")),
        stroke: map_figma_stroke(figma),
        effects: map_figma_node_effects(figma),
        ..ContainerProps::default()
    };
    rectangle_node(common_props(figma, id), container)
}

/// Figma arc (radians, start+end) → Pen arc (degrees, start+sweep).
fn map_figma_arc_data(arc: &FigValue) -> (Option<f64>, Option<f64>, Option<f64>) {
    let start_rad = arc.get_f64("startingAngle").unwrap_or(0.0);
    let end_rad = arc
        .get_f64("endingAngle")
        .unwrap_or(std::f64::consts::PI * 2.0);
    let inner = arc.get_f64("innerRadius").unwrap_or(0.0);
    let (actual_start, sweep_rad) = if end_rad >= start_rad {
        (start_rad, end_rad - start_rad)
    } else {
        (end_rad, start_rad - end_rad)
    };
    let start_deg = actual_start * 180.0 / std::f64::consts::PI;
    let sweep_deg = sweep_rad * 180.0 / std::f64::consts::PI;
    (
        if start_deg.abs() > 0.1 {
            Some(round2(start_deg))
        } else {
            None
        },
        if (sweep_deg - 360.0).abs() > 0.1 {
            Some(round2(sweep_deg))
        } else {
            None
        },
        if inner > 0.001 {
            Some(round3(inner))
        } else {
            None
        },
    )
}

fn convert_ellipse(
    tree: &TreeNode,
    parent_stack_mode: Option<&str>,
    ctx: &mut ConversionContext<'_>,
) -> PenNode {
    let id = ctx.generate_id();
    let figma = &tree.figma;
    let width = resolve_width(figma, parent_stack_mode, ctx);
    let height = resolve_height(figma, parent_stack_mode, ctx);
    let mut base = common_props(figma, id);

    let (mut start_angle, mut sweep_angle, inner_radius) = figma
        .get("arcData")
        .map(map_figma_arc_data)
        .unwrap_or((None, None, None));

    // Absorb a flip into the arc angles (extract_flip sets at most one).
    if start_angle.is_some() || sweep_angle.is_some() || inner_radius.is_some() {
        let start = start_angle.unwrap_or(0.0);
        let sweep = sweep_angle.unwrap_or(360.0);
        if base.flip_x == Some(true) {
            start_angle = Some(normalize_angle(180.0 - start - sweep));
            sweep_angle = Some(sweep);
            base.flip_x = None;
        }
        if base.flip_y == Some(true) {
            start_angle = Some(normalize_angle(360.0 - start - sweep));
            sweep_angle = Some(sweep);
            base.flip_y = None;
        }
    }

    ellipse_node(
        base,
        width,
        height,
        map_figma_fills(figma.get_array("fillPaints")),
        map_figma_stroke(figma),
        map_figma_node_effects(figma),
        start_angle,
        sweep_angle,
        inner_radius,
    )
}

fn convert_line(tree: &TreeNode, ctx: &mut ConversionContext<'_>) -> PenNode {
    let id = ctx.generate_id();
    let figma = &tree.figma;
    let size = figma.get("size");
    let w = size.and_then(|s| s.get_f64("x")).unwrap_or(100.0);
    let h = size.and_then(|s| s.get_f64("y")).unwrap_or(0.0);
    // Line builds its base manually — no flip / locked.
    let mut base = common_props(figma, id);
    base.flip_x = None;
    base.flip_y = None;
    base.locked = None;
    line_node(
        base,
        w,
        h,
        map_figma_stroke(figma),
        map_figma_node_effects(figma),
    )
}

/// Whether the node references enough binary data to represent actual
/// vector geometry. Short command blobs and nodes with no geometry
/// source are degenerate, while non-empty undecodable data still uses
/// the visible rectangle fallback so the import failure remains clear.
fn has_non_degenerate_vector_geometry(figma: &FigValue, ctx: &ConversionContext<'_>) -> bool {
    for key in ["fillGeometry", "strokeGeometry"] {
        if let Some(geometries) = figma.get_array(key) {
            for geometry in geometries {
                let Some(index) = geometry.get_f64("commandsBlob") else {
                    continue;
                };
                if ctx
                    .blobs
                    .get(index as usize)
                    .and_then(|blob| blob.byte_len())
                    .is_some_and(|length| length >= 9)
                {
                    return true;
                }
            }
        }
    }

    figma
        .get("vectorData")
        .and_then(|data| data.get_f64("vectorNetworkBlob"))
        .and_then(|index| ctx.blobs.get(index as usize))
        .and_then(|blob| blob.byte_len())
        .is_some_and(|length| length >= 9)
}

fn convert_vector(
    tree: &TreeNode,
    parent_stack_mode: Option<&str>,
    ctx: &mut ConversionContext<'_>,
) -> PenNode {
    let id = ctx.generate_id();
    let figma = &tree.figma;

    if let Some(group) = convert_swapped_boolean_group(tree, parent_stack_mode, id.clone(), ctx) {
        return group;
    }
    // Resolve matching host icons to canonical 24×24 path data.
    let name = figma.get_str("name").unwrap_or("");
    if let Some(icon) = lookup_icon_by_name(name) {
        return build_icon_path_node(figma, id, parent_stack_mode, ctx, icon);
    }

    let DecodedVectorPath {
        d: path_d,
        fill_rule,
        allows_fill,
        from_stroke_geometry,
    } = decode_figma_vector_path(figma, &ctx.blobs).unwrap_or_default();

    if !path_d.is_empty() {
        let mut base = common_props(figma, id);
        let mut width = resolve_width(figma, parent_stack_mode, ctx);
        let mut height = resolve_height(figma, parent_stack_mode, ctx);

        // Zero-size vectors: derive extent from the path bounds.
        let size_x = figma
            .get("size")
            .and_then(|s| s.get_f64("x"))
            .unwrap_or(0.0);
        let size_y = figma
            .get("size")
            .and_then(|s| s.get_f64("y"))
            .unwrap_or(0.0);
        if (size_x < 0.01 || size_y < 0.01)
            && matches!(width, SizingBehavior::Number(_))
            && matches!(height, SizingBehavior::Number(_))
        {
            if let Some(bounds) = compute_svg_path_bounds(&path_d) {
                let path_w = bounds.max_x - bounds.min_x;
                let path_h = bounds.max_y - bounds.min_y;
                if size_x < 0.01 && path_w > 0.01 {
                    width = SizingBehavior::Number(round2(path_w));
                    base.x = Some(round2(base.x.unwrap_or(0.0) + bounds.min_x));
                }
                if size_y < 0.01 && path_h > 0.01 {
                    height = SizingBehavior::Number(round2(path_h));
                    base.y = Some(round2(base.y.unwrap_or(0.0) + bounds.min_y));
                }
            }
        }

        // Stroke-only outline: Figma strokeGeometry is an expanded
        // outline, so treat the stroke paints as the fill.
        let has_visible_fills = any_visible(figma.get_array("fillPaints"));
        let has_visible_strokes = any_visible(figma.get_array("strokePaints"));
        // Only a path that genuinely CAME from strokeGeometry is the
        // pre-expanded outline; a vector-network fallback is a
        // centerline and must keep its stroke.
        let has_authored_fills = figma
            .get_array("fillPaints")
            .is_some_and(|paints| !paints.is_empty());
        let has_authored_strokes = figma
            .get_array("strokePaints")
            .is_some_and(|paints| !paints.is_empty());
        let is_stroke_only = !has_visible_fills
            && (has_visible_strokes || (!has_authored_fills && has_authored_strokes))
            && from_stroke_geometry;

        if is_stroke_only {
            return path_node(
                base,
                Some(path_d),
                None,
                fill_rule,
                width,
                height,
                map_figma_fills(figma.get_array("strokePaints")),
                None,
                map_figma_node_effects(figma),
                figma_path_mask(figma),
            );
        }
        return path_node(
            base,
            Some(path_d),
            None,
            fill_rule,
            width,
            height,
            allows_fill
                .then(|| map_figma_fills(figma.get_array("fillPaints")))
                .flatten(),
            map_figma_stroke(figma),
            map_figma_node_effects(figma),
            figma_path_mask(figma),
        );
    }

    if let Some((decoded, stroke)) = decode_single_child_boolean(tree, &ctx.blobs) {
        return path_node(
            common_props(figma, id),
            Some(decoded.d),
            None,
            decoded.fill_rule,
            resolve_width(figma, parent_stack_mode, ctx),
            resolve_height(figma, parent_stack_mode, ctx),
            None,
            Some(stroke),
            map_figma_node_effects(figma),
            figma_path_mask(figma),
        );
    }

    if !has_non_degenerate_vector_geometry(figma, ctx) {
        if let Some(group) = convert_empty_boolean_group(tree, parent_stack_mode, id.clone(), ctx) {
            return group;
        }
        ctx.warnings.push(format!(
            "Vector node \"{}\" imported as invisible path (empty geometry)",
            figma.get_str("name").unwrap_or("")
        ));
        return path_node(
            common_props(figma, id),
            None,
            None,
            None,
            resolve_width(figma, parent_stack_mode, ctx),
            resolve_height(figma, parent_stack_mode, ctx),
            None,
            None,
            None,
            figma_path_mask(figma),
        );
    }

    // Fallback — genuinely non-empty but undecodable vector data stays
    // visible as a rectangle and retains the stronger warning class.
    ctx.warnings.push(format!(
        "Vector node \"{}\" converted as rectangle (path data not decodable)",
        figma.get_str("name").unwrap_or("")
    ));
    let container = ContainerProps {
        width: Some(resolve_width(figma, parent_stack_mode, ctx)),
        height: Some(resolve_height(figma, parent_stack_mode, ctx)),
        fill: map_figma_fills(figma.get_array("fillPaints")),
        stroke: map_figma_stroke(figma),
        effects: map_figma_node_effects(figma),
        ..ContainerProps::default()
    };
    rectangle_node(common_props(figma, id), container)
}

mod icon;
use icon::build_icon_path_node;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_boolean_fallback;
#[cfg(test)]
mod tests_instance_scale;
#[cfg(test)]
mod tests_instance_swap;
#[cfg(test)]
mod tests_vector_regressions;
