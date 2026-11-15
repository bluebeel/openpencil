//! Paint bounds with sibling masks, without treating mask sources as visible nodes.

use super::*;

/// SVG export counts gradient and image fills as own paint.
const SVG_PAINT_RULES: PaintCornerRules = PaintCornerRules {
    gradient_paints: true,
    image_paints: true,
};

pub(super) fn collect_bounds(n: &SceneNode, parent_xform: glam::Affine2, acc: &mut BoundsAcc) {
    if n.hidden {
        return;
    }
    let local_xform = parent_xform * node_transform(n);
    for_each_own_paint_corner(n, |p| {
        let w = local_xform.transform_point2(p);
        acc.add(w.x, w.y, w.x, w.y);
    });
    if n.clip_content && !n.children.is_empty() && n.bounds.size.x > 0.0 && n.bounds.size.y > 0.0 {
        let mut child_acc = BoundsAcc::new();
        collect_sibling_bounds(n.visible_children(), local_xform, &mut child_acc);
        if let Some(visible) = child_acc
            .into_rect()
            .and_then(|bounds| apply_clip_bounds(bounds, n.bounds, local_xform))
        {
            acc.add(
                visible.origin.x,
                visible.origin.y,
                visible.origin.x + visible.size.x,
                visible.origin.y + visible.size.y,
            );
        }
        return;
    }
    collect_sibling_bounds(n.visible_children(), local_xform, acc);
}

fn for_each_own_paint_corner(n: &SceneNode, mut visit: impl FnMut(glam::Vec2)) {
    if composite::has_widget_visual(n) {
        // An empty input still paints its surface, placeholder and icons.
        // Its degraded Text kind must not make the export appear empty.
        let r = normalize_rect(n.bounds);
        let pad = n.stroke.map_or(0.0, |stroke| stroke.width.max(0.0) / 2.0);
        for point in [
            glam::Vec2::new(r.origin.x - pad, r.origin.y - pad),
            glam::Vec2::new(r.origin.x + r.size.x + pad, r.origin.y - pad),
            glam::Vec2::new(r.origin.x + r.size.x + pad, r.origin.y + r.size.y + pad),
            glam::Vec2::new(r.origin.x - pad, r.origin.y + r.size.y + pad),
        ] {
            visit(point);
        }
        return;
    }
    crate::scene_bounds::for_each_own_paint_corner(n, SVG_PAINT_RULES, visit);
}

/// Mask markers split runs even when hidden or unsupported, exactly as paint.
/// Bounds are conservative paint AABBs, then intersected with mask coverage.
pub(super) fn collect_sibling_bounds(
    nodes: &[SceneNode],
    parent: glam::Affine2,
    out: &mut BoundsAcc,
) {
    let mut run = BoundsAcc::new();
    let mut mask = None;
    for node in nodes.iter().rev() {
        if masks::marker(node).is_some() {
            finish_run(std::mem::take(&mut run), mask, parent, out);
            mask = masks::operation(node).map(|_| node);
        } else {
            collect_bounds(node, parent, &mut run);
        }
    }
    finish_run(run, mask, parent, out);
}

fn finish_run(
    run: BoundsAcc,
    mask: Option<&SceneNode>,
    parent: glam::Affine2,
    out: &mut BoundsAcc,
) {
    let Some(mut bounds) = run.into_rect() else {
        return;
    };
    if let Some(mask) = mask {
        let Some(source) = masks::source_bounds(mask, parent) else {
            return;
        };
        let Some(intersection) = transform::intersect(bounds, source) else {
            return;
        };
        bounds = intersection;
    }
    out.add(
        bounds.origin.x,
        bounds.origin.y,
        bounds.origin.x + bounds.size.x,
        bounds.origin.y + bounds.size.y,
    );
}
