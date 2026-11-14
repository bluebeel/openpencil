//! Conservative offscreen-layer bounds for canvas compositing.
//!
//! Node blend modes and Figma sibling masks need save-layers, but allocating
//! those layers for the whole viewport is unnecessarily expensive. These
//! helpers bound the painted subtree (including strokes, shadows, and layer
//! blur), transform it into viewport coordinates, and intersect it with the
//! caller's cull rect. Invalid geometry falls back to the cull rect so a bad
//! imported value cannot turn into an unbounded or NaN allocation hint.

use super::shadow_geometry::spread_rect;
use super::OverlayTransform;
use crate::layout_scene::{Effect, NodeKind, SceneNode, SceneStrokeAlign};
use crate::{Point2D, Rect};

/// Skia's practical Gaussian support is three sigma on either side. Both
/// scene blur radii and drop-shadow blur values are converted to sigma by
/// dividing by two, hence `radius * 1.5` below.
const FILTER_OUTSET_PER_RADIUS: f32 = 1.5;
/// Preserve the antialiased fringe at an otherwise exact layer edge.
const DEVICE_EDGE_PAD: f32 = 1.0;
const MIN_LAYER_EXTENT: f32 = 1.0;

type BoundsResult = Result<Option<Rect>, ()>;

/// Bounds for a node-level blend layer. The caller has already installed the
/// root node's transform, so only descendant transforms are folded into this
/// local-coordinate allocation hint.
pub(super) fn node_composite_layer_bounds(
    node: &SceneNode,
    viewport_origin: Point2D,
    zoom: f32,
    cull: Rect,
    hidden_id: Option<&str>,
    transforms: &[OverlayTransform],
) -> Rect {
    finish_layer_bounds(
        subtree_visual_bounds(node, false, hidden_id, inverse_shadow_basis(transforms)),
        viewport_origin,
        zoom,
        cull,
        transforms,
    )
}

/// Bounds for one sibling-mask run. Mask and content roots have not installed
/// their own transforms yet, so each root transform is included. `content`
/// must contain only the front siblings governed by this mask (the mask
/// painter stops the iterator at the next mask).
pub(super) fn sibling_mask_layer_bounds<'a>(
    mask: &SceneNode,
    content: impl IntoIterator<Item = &'a SceneNode>,
    viewport_origin: Point2D,
    zoom: f32,
    cull: Rect,
    hidden_id: Option<&str>,
    transforms: &[OverlayTransform],
) -> Rect {
    let mut bounds = subtree_visual_bounds(mask, true, hidden_id, inverse_shadow_basis(transforms));
    for node in content {
        bounds = union_results(
            bounds,
            subtree_visual_bounds(node, true, hidden_id, inverse_shadow_basis(transforms)),
        );
    }
    finish_layer_bounds(bounds, viewport_origin, zoom, cull, transforms)
}

/// Whether any paint from `node` or its descendants can reach `cull`.
///
/// Unlike the old leaf-only canvas cull, this uses the same conservative
/// effect- and transform-aware subtree bounds as offscreen layer allocation.
/// Invalid imported geometry stays paintable rather than disappearing.
pub(super) fn subtree_intersects_cull(
    node: &SceneNode,
    viewport_origin: Point2D,
    zoom: f32,
    cull: Rect,
    hidden_id: Option<&str>,
    transforms: &[OverlayTransform],
) -> bool {
    let cull = match finite_normalized_rect(cull) {
        Ok(Some(cull)) => cull,
        _ => return true,
    };
    let local_cull = match inverse_transform_rect(cull, transforms) {
        Ok(cull) => cull,
        Err(()) => return true,
    };
    let doc_bounds =
        match subtree_visual_bounds(node, true, hidden_id, inverse_shadow_basis(transforms)) {
            Ok(Some(bounds)) => bounds,
            Ok(None) => return false,
            Err(()) => return true,
        };
    let world_bounds = match doc_to_world_rect(doc_bounds, viewport_origin, zoom)
        .and_then(|bounds| outset_rect(bounds, DEVICE_EDGE_PAD))
    {
        Ok(bounds) => bounds,
        Err(()) => return true,
    };
    intersect_rects(world_bounds, local_cull).is_some()
}

fn subtree_visual_bounds(
    node: &SceneNode,
    apply_root_transform: bool,
    hidden_id: Option<&str>,
    world_to_parent: glam::Mat2,
) -> BoundsResult {
    subtree_visual_bounds_inner(
        node,
        apply_root_transform,
        hidden_id,
        world_to_parent,
        false,
    )
}

fn subtree_visual_bounds_inner(
    node: &SceneNode,
    apply_root_transform: bool,
    hidden_id: Option<&str>,
    world_to_parent: glam::Mat2,
    selected_output: bool,
) -> BoundsResult {
    if node.hidden || hidden_id == Some(node.id.as_str()) {
        return Ok(None);
    }

    let world_to_local = if apply_root_transform
        && (node.flip_x || node.flip_y || node.rotation.abs() > f32::EPSILON)
    {
        inverse_node_basis(node.rotation, node.flip_x, node.flip_y) * world_to_parent
    } else {
        world_to_parent
    };
    let own_bounds = finite_normalized_rect(node.bounds)?;
    let owns_geometry = !selected_output
        || node.widget.is_some()
        || node.image_src.is_some()
        || crate::scene_bounds::own_paint_corners(
            node,
            crate::scene_bounds::PaintCornerRules {
                gradient_paints: true,
                image_paints: true,
            },
        )
        .is_some();
    let mut visual = if owns_geometry {
        own_bounds
            .map(|bounds| stroke_visual_bounds(node, bounds))
            .transpose()?
    } else {
        None
    };
    if selected_output {
        let recurses = matches!(
            node.kind,
            NodeKind::Frame | NodeKind::Group | NodeKind::Rect
        ) || matches!(&node.kind, NodeKind::Other(tag) if tag != "icon_font");
        let mut children = if recurses {
            export_children_bounds(node.visible_children(), hidden_id, world_to_local)?
        } else {
            None
        };
        if node.clip_content
            && !node.children.is_empty()
            && node.bounds.size.x > 0.0
            && node.bounds.size.y > 0.0
        {
            children =
                children.and_then(|child| own_bounds.and_then(|clip| intersect_rects(child, clip)));
        }
        visual = union_options(visual, children)?;
    } else {
        for child in &node.children {
            visual = union_results(
                Ok(visual),
                subtree_visual_bounds(child, true, hidden_id, world_to_local),
            )?;
        }
    }

    // The painter currently casts outer shadows only for shapes whose
    // silhouette can be represented by its rounded-rect/ellipse primitive.
    if matches!(
        node.kind,
        NodeKind::Frame | NodeKind::Rect | NodeKind::Ellipse
    ) {
        if let Some(own) = own_bounds {
            for effect in &node.effects {
                let Effect::DropShadow(shadow) = effect else {
                    continue;
                };
                if shadow.inner {
                    continue;
                }
                if !shadow.offset_x.is_finite()
                    || !shadow.offset_y.is_finite()
                    || !shadow.blur.is_finite()
                    || !shadow.spread.is_finite()
                {
                    return Err(());
                }
                let Some(silhouette) = spread_rect(own, shadow.spread) else {
                    continue;
                };
                let offset = world_to_local * glam::Vec2::new(shadow.offset_x, shadow.offset_y);
                let mut shadow_bounds = translate_rect(silhouette, offset.x, offset.y)?;
                if shadow.blur > 0.0 {
                    shadow_bounds =
                        outset_rect(shadow_bounds, shadow.blur * FILTER_OUTSET_PER_RADIUS)?;
                }
                visual = union_options(visual, Some(shadow_bounds))?;
            }
        }
    }

    // Layer blur captures shadows, own paint, and every descendant, so its
    // filter support expands the completed union rather than only own bounds.
    for effect in &node.effects {
        match effect {
            Effect::Blur(blur) => {
                if !blur.radius.is_finite() {
                    return Err(());
                }
                if blur.radius > 0.0 {
                    visual = visual
                        .map(|bounds| outset_rect(bounds, blur.radius * FILTER_OUTSET_PER_RADIUS))
                        .transpose()?;
                }
            }
            Effect::BackgroundBlur { radius } => {
                // Backdrop blur is clipped to the node silhouette and does
                // not extend the node's output, but reject non-finite input.
                if !radius.is_finite() {
                    return Err(());
                }
            }
            Effect::DropShadow(_) => {}
        }
    }

    let transformed = node.flip_x || node.flip_y || node.rotation.abs() > f32::EPSILON;
    if apply_root_transform && transformed {
        if !node.rotation.is_finite() {
            return Err(());
        }
        let Some(bounds) = visual else {
            return Ok(None);
        };
        let pivot_bounds = match finite_normalized_rect(node.aggregate_bounds())? {
            Some(bounds) => bounds,
            None => bounds,
        };
        let pivot = Point2D::new(
            pivot_bounds.origin.x + pivot_bounds.size.x * 0.5,
            pivot_bounds.origin.y + pivot_bounds.size.y * 0.5,
        );
        visual = Some(transformed_aabb(
            bounds,
            node.flip_x,
            node.flip_y,
            node.rotation,
            pivot,
        )?);
    }

    Ok(visual)
}

// Shadow displacements use canvas axes even under ancestor transforms. Carry
// only the inverse linear basis; translations do not affect these vectors.
fn inverse_shadow_basis(transforms: &[OverlayTransform]) -> glam::Mat2 {
    transforms
        .iter()
        .fold(glam::Mat2::IDENTITY, |basis, transform| {
            inverse_node_basis(transform.rotation, transform.flip_x, transform.flip_y) * basis
        })
}

fn inverse_node_basis(rotation: f32, flip_x: bool, flip_y: bool) -> glam::Mat2 {
    glam::Mat2::from_angle(-rotation)
        * glam::Mat2::from_diagonal(glam::Vec2::new(
            if flip_x { -1.0 } else { 1.0 },
            if flip_y { -1.0 } else { 1.0 },
        ))
}

fn stroke_visual_bounds(node: &SceneNode, bounds: Rect) -> Result<Rect, ()> {
    let Some(stroke) = node.stroke else {
        // Lines and unfilled open paths get a small implicit stroke in the
        // painter. Device padding below covers the normal case; this doc-space
        // allowance also protects high zoom values.
        return if matches!(node.kind, NodeKind::Line | NodeKind::Path) {
            outset_rect(bounds, 1.0)
        } else {
            Ok(bounds)
        };
    };
    let mut width = stroke.width;
    if let Some(sides) = stroke.sides {
        for side in sides {
            if !side.is_finite() {
                return Err(());
            }
            width = width.max(side);
        }
    }
    if !width.is_finite() {
        return Err(());
    }
    let width = width.max(0.0);
    let outset = match stroke.align {
        SceneStrokeAlign::Inside => 0.0,
        SceneStrokeAlign::Center => width * 0.5,
        SceneStrokeAlign::Outside => width,
    };
    outset_rect(
        bounds,
        if node.stroke_paint.is_some() {
            outset.max(width * 4.0)
        } else {
            outset
        },
    )
}

fn finish_layer_bounds(
    doc_bounds: BoundsResult,
    viewport_origin: Point2D,
    zoom: f32,
    cull: Rect,
    transforms: &[OverlayTransform],
) -> Rect {
    let cull = match finite_normalized_rect(cull) {
        Ok(Some(cull)) => cull,
        _ => return Rect::xywh(0.0, 0.0, MIN_LAYER_EXTENT, MIN_LAYER_EXTENT),
    };
    let cull = inverse_transform_rect(cull, transforms).unwrap_or(cull);
    let doc_bounds = match doc_bounds {
        Ok(Some(bounds)) => bounds,
        Ok(None) => return minimum_rect_inside(cull),
        Err(()) => return cull,
    };
    let world = match doc_to_world_rect(doc_bounds, viewport_origin, zoom)
        .and_then(|bounds| outset_rect(bounds, DEVICE_EDGE_PAD))
    {
        Ok(bounds) => bounds,
        Err(()) => return cull,
    };
    intersect_rects(world, cull).unwrap_or_else(|| minimum_rect_inside(cull))
}

fn finite_normalized_rect(rect: Rect) -> Result<Option<Rect>, ()> {
    let values = [rect.origin.x, rect.origin.y, rect.size.x, rect.size.y];
    if values.iter().any(|value| !value.is_finite()) {
        return Err(());
    }
    let x2 = rect.origin.x + rect.size.x;
    let y2 = rect.origin.y + rect.size.y;
    if !x2.is_finite() || !y2.is_finite() {
        return Err(());
    }
    let min_x = rect.origin.x.min(x2);
    let min_y = rect.origin.y.min(y2);
    let max_x = rect.origin.x.max(x2);
    let max_y = rect.origin.y.max(y2);
    if max_x <= min_x && max_y <= min_y {
        return Ok(None);
    }
    Ok(Some(Rect::xywh(min_x, min_y, max_x - min_x, max_y - min_y)))
}

fn union_results(left: BoundsResult, right: BoundsResult) -> BoundsResult {
    union_options(left?, right?)
}

fn union_options(left: Option<Rect>, right: Option<Rect>) -> Result<Option<Rect>, ()> {
    match (left, right) {
        (None, None) => Ok(None),
        (Some(rect), None) | (None, Some(rect)) => Ok(Some(rect)),
        (Some(left), Some(right)) => {
            let min_x = left.origin.x.min(right.origin.x);
            let min_y = left.origin.y.min(right.origin.y);
            let max_x = (left.origin.x + left.size.x).max(right.origin.x + right.size.x);
            let max_y = (left.origin.y + left.size.y).max(right.origin.y + right.size.y);
            finite_normalized_rect(Rect::xywh(min_x, min_y, max_x - min_x, max_y - min_y))
        }
    }
}

fn outset_rect(rect: Rect, amount: f32) -> Result<Rect, ()> {
    if !amount.is_finite() || amount < 0.0 {
        return Err(());
    }
    let result = Rect::xywh(
        rect.origin.x - amount,
        rect.origin.y - amount,
        rect.size.x + amount * 2.0,
        rect.size.y + amount * 2.0,
    );
    finite_normalized_rect(result)?.ok_or(())
}

fn translate_rect(rect: Rect, dx: f32, dy: f32) -> Result<Rect, ()> {
    let result = Rect::xywh(
        rect.origin.x + dx,
        rect.origin.y + dy,
        rect.size.x,
        rect.size.y,
    );
    finite_normalized_rect(result)?.ok_or(())
}

fn transformed_aabb(
    rect: Rect,
    flip_x: bool,
    flip_y: bool,
    radians: f32,
    pivot: Point2D,
) -> Result<Rect, ()> {
    if !radians.is_finite() || !pivot.x.is_finite() || !pivot.y.is_finite() {
        return Err(());
    }
    let (sin, cos) = radians.sin_cos();
    let transform = |x: f32, y: f32| {
        // Canvas concatenates flip then rotation as F*R: points rotate first,
        // then mirror. The root blend caller already installed both.
        let dx = x - pivot.x;
        let dy = y - pivot.y;
        Point2D::new(
            pivot.x + (dx * cos - dy * sin) * if flip_x { -1.0 } else { 1.0 },
            pivot.y + (dx * sin + dy * cos) * if flip_y { -1.0 } else { 1.0 },
        )
    };
    let x2 = rect.origin.x + rect.size.x;
    let y2 = rect.origin.y + rect.size.y;
    let points = [
        transform(rect.origin.x, rect.origin.y),
        transform(x2, rect.origin.y),
        transform(x2, y2),
        transform(rect.origin.x, y2),
    ];
    if points
        .iter()
        .any(|point| !point.x.is_finite() || !point.y.is_finite())
    {
        return Err(());
    }
    let min_x = points
        .iter()
        .map(|point| point.x)
        .fold(f32::INFINITY, f32::min);
    let min_y = points
        .iter()
        .map(|point| point.y)
        .fold(f32::INFINITY, f32::min);
    let max_x = points
        .iter()
        .map(|point| point.x)
        .fold(f32::NEG_INFINITY, f32::max);
    let max_y = points
        .iter()
        .map(|point| point.y)
        .fold(f32::NEG_INFINITY, f32::max);
    finite_normalized_rect(Rect::xywh(min_x, min_y, max_x - min_x, max_y - min_y))?.ok_or(())
}

/// `cull` arrives in page/screen coordinates while a save-layer bound is
/// interpreted in the canvas's current coordinate system. Undo the active
/// flip/rotation chain (child first) before intersecting the two. Taking an
/// AABB after each inverse step is conservative for nested rotations.
fn inverse_transform_rect(mut rect: Rect, transforms: &[OverlayTransform]) -> Result<Rect, ()> {
    for transform in transforms.iter().rev() {
        if !transform.rotation.is_finite()
            || !transform.pivot.x.is_finite()
            || !transform.pivot.y.is_finite()
        {
            return Err(());
        }
        let (sin, cos) = transform.rotation.sin_cos();
        let inverse = |x: f32, y: f32| {
            // Undo F*R by mirroring first, then rotating back.
            let dx = (x - transform.pivot.x) * if transform.flip_x { -1.0 } else { 1.0 };
            let dy = (y - transform.pivot.y) * if transform.flip_y { -1.0 } else { 1.0 };
            Point2D::new(
                transform.pivot.x + dx * cos + dy * sin,
                transform.pivot.y - dx * sin + dy * cos,
            )
        };
        let x2 = rect.origin.x + rect.size.x;
        let y2 = rect.origin.y + rect.size.y;
        let points = [
            inverse(rect.origin.x, rect.origin.y),
            inverse(x2, rect.origin.y),
            inverse(x2, y2),
            inverse(rect.origin.x, y2),
        ];
        if points
            .iter()
            .any(|point| !point.x.is_finite() || !point.y.is_finite())
        {
            return Err(());
        }
        let min_x = points
            .iter()
            .map(|point| point.x)
            .fold(f32::INFINITY, f32::min);
        let min_y = points
            .iter()
            .map(|point| point.y)
            .fold(f32::INFINITY, f32::min);
        let max_x = points
            .iter()
            .map(|point| point.x)
            .fold(f32::NEG_INFINITY, f32::max);
        let max_y = points
            .iter()
            .map(|point| point.y)
            .fold(f32::NEG_INFINITY, f32::max);
        rect = finite_normalized_rect(Rect::xywh(min_x, min_y, max_x - min_x, max_y - min_y))?
            .ok_or(())?;
    }
    Ok(rect)
}

fn doc_to_world_rect(rect: Rect, viewport_origin: Point2D, zoom: f32) -> Result<Rect, ()> {
    if !viewport_origin.x.is_finite()
        || !viewport_origin.y.is_finite()
        || !zoom.is_finite()
        || zoom <= 0.0
    {
        return Err(());
    }
    finite_normalized_rect(Rect::xywh(
        viewport_origin.x + rect.origin.x * zoom,
        viewport_origin.y + rect.origin.y * zoom,
        rect.size.x * zoom,
        rect.size.y * zoom,
    ))?
    .ok_or(())
}

fn intersect_rects(left: Rect, right: Rect) -> Option<Rect> {
    let min_x = left.origin.x.max(right.origin.x);
    let min_y = left.origin.y.max(right.origin.y);
    let max_x = (left.origin.x + left.size.x).min(right.origin.x + right.size.x);
    let max_y = (left.origin.y + left.size.y).min(right.origin.y + right.size.y);
    (max_x > min_x && max_y > min_y).then(|| Rect::xywh(min_x, min_y, max_x - min_x, max_y - min_y))
}

fn minimum_rect_inside(cull: Rect) -> Rect {
    Rect::xywh(
        cull.origin.x,
        cull.origin.y,
        cull.size.x.clamp(f32::EPSILON, MIN_LAYER_EXTENT),
        cull.size.y.clamp(f32::EPSILON, MIN_LAYER_EXTENT),
    )
}

#[cfg(test)]
#[path = "canvas_viewport_layer_bounds_tests.rs"]
mod tests;

/// Selected export surfaces carry a page-space cull through ancestor CTMs.
pub(super) fn export_layer_bounds(cull: Rect, transforms: &[OverlayTransform]) -> Rect {
    inverse_transform_rect(cull, transforms).unwrap_or(cull)
}

pub(super) fn export_source_bounds(
    node: &SceneNode,
    transforms: &[OverlayTransform],
) -> Option<Rect> {
    subtree_visual_bounds_inner(node, true, None, inverse_shadow_basis(transforms), true)
        .ok()
        .flatten()
}

/// Apply only ancestor effects on the selected branch; ancestor own paint and
/// outer shadows are intentionally absent from a selected-subtree export.
pub(super) fn export_ancestor_bounds(node: &SceneNode, mut bounds: Rect) -> Option<Rect> {
    if node.clip_content
        && !node.children.is_empty()
        && node.bounds.size.x > 0.0
        && node.bounds.size.y > 0.0
    {
        bounds = intersect_rects(bounds, finite_normalized_rect(node.bounds).ok()??)?;
    }
    if let Some(radius) = node.effects.iter().find_map(|effect| match effect {
        Effect::Blur(blur) if blur.radius > 0.0 => Some(blur.radius),
        _ => None,
    }) {
        bounds = outset_rect(bounds, radius * FILTER_OUTSET_PER_RADIUS).ok()?;
    }
    if node.flip_x || node.flip_y || node.rotation.abs() > f32::EPSILON {
        let pivot = node.aggregate_bounds();
        bounds = transformed_aabb(
            bounds,
            node.flip_x,
            node.flip_y,
            node.rotation,
            pivot.origin + pivot.size * 0.5,
        )
        .ok()?;
    }
    Some(bounds)
}

pub(super) fn export_intersection(a: Rect, b: Rect) -> Option<Rect> {
    intersect_rects(a, b)
}

fn export_children_bounds(
    nodes: &[SceneNode],
    hidden: Option<&str>,
    basis: glam::Mat2,
) -> BoundsResult {
    let mut out = None;
    let mut run = None;
    let mut mask = None;
    for node in nodes.iter().rev() {
        if super::mask::mask_type(node).is_some() {
            out = union_options(out, export_masked_run(run, mask, hidden, basis)?)?;
            run = None;
            mask = super::mask::export_mask_supported(node).then_some(node);
        } else {
            run = union_options(
                run,
                subtree_visual_bounds_inner(node, true, hidden, basis, true)?,
            )?;
        }
    }
    union_options(out, export_masked_run(run, mask, hidden, basis)?)
}

fn export_masked_run(
    run: Option<Rect>,
    mask: Option<&SceneNode>,
    hidden: Option<&str>,
    basis: glam::Mat2,
) -> BoundsResult {
    let Some(run) = run else { return Ok(None) };
    if let Some(mask) = mask {
        let source = if hidden == Some(mask.id.as_str()) {
            None
        } else {
            export_mask_bounds_inner(mask, basis)?
        };
        Ok(source.and_then(|source| intersect_rects(run, source)))
    } else {
        Ok(Some(run))
    }
}

pub(super) fn export_mask_bounds(
    node: &SceneNode,
    transforms: &[OverlayTransform],
) -> Option<Rect> {
    export_mask_bounds_inner(node, inverse_shadow_basis(transforms))
        .ok()
        .flatten()
}

fn export_mask_bounds_inner(node: &SceneNode, basis: glam::Mat2) -> BoundsResult {
    if !super::mask::export_mask_is_geometry_clip(node) {
        return subtree_visual_bounds_inner(node, true, None, basis, true);
    }
    let bounds = if node
        .svg_path
        .as_deref()
        .is_some_and(|d| !d.trim().is_empty())
    {
        finite_normalized_rect(node.bounds)?
    } else {
        let points = jian_scene::path_geometry::flatten_path_points(node);
        let mut bounds: Option<Rect> = None;
        for point in points.as_slice() {
            // One-pixel rectangles are not used: retaining zero-area extrema
            // here keeps a vector clip's actual geometric bounding box.
            if !point.x.is_finite() || !point.y.is_finite() {
                return Err(());
            }
            bounds = Some(match bounds {
                None => Rect::xywh(point.x, point.y, 0.0, 0.0),
                Some(r) => {
                    let x0 = r.origin.x.min(point.x);
                    let y0 = r.origin.y.min(point.y);
                    let x1 = (r.origin.x + r.size.x).max(point.x);
                    let y1 = (r.origin.y + r.size.y).max(point.y);
                    Rect::xywh(x0, y0, x1 - x0, y1 - y0)
                }
            });
        }
        bounds
    };
    let Some(bounds) = bounds else {
        return Ok(None);
    };
    if node.flip_x || node.flip_y || node.rotation.abs() > f32::EPSILON {
        let pivot = node.aggregate_bounds();
        transformed_aabb(
            bounds,
            node.flip_x,
            node.flip_y,
            node.rotation,
            pivot.origin + pivot.size * 0.5,
        )
        .map(Some)
    } else {
        Ok(Some(bounds))
    }
}
