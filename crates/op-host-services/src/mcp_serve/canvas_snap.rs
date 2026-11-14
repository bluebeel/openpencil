//! Resolve one snap scope to a borrowed scene traversal and a reusable axis index.
use op_editor_ui::{
    layout_scene::{LayoutScene, SceneNode},
    snapping::{SnapIndex, SnapRect, SnapTarget},
};

pub(super) fn index(scene: &LayoutScene, parent: Option<&str>) -> Option<SnapIndex> {
    let page = scene.active_page()?;
    let mut ancestors = Vec::new();
    let nodes = match parent {
        None => page.children.as_slice(),
        Some(id) => {
            let node = find(&page.children, id, &mut ancestors)?;
            if node.hidden || ancestors.iter().any(|ancestor| ancestor.hidden) {
                return Some(SnapIndex::new(std::iter::empty::<SnapTarget>()));
            }
            ancestors.push(node);
            node.children.as_slice()
        }
    };
    Some(SnapIndex::new(nodes.iter().filter(|n| !n.hidden).map(
        |node| {
            let bounds = node.aggregate_bounds();
            let left = bounds.origin.x;
            let top = bounds.origin.y;
            let right = left + bounds.size.x;
            let bottom = top + bounds.size.y;
            let mut corners = [(left, top), (right, top), (right, bottom), (left, bottom)];
            for current in std::iter::once(node).chain(ancestors.iter().rev().copied()) {
                for point in &mut corners {
                    *point = transform(*point, current);
                }
            }
            let min_x = corners.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
            let min_y = corners.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
            let max_x = corners
                .iter()
                .map(|p| p.0)
                .fold(f32::NEG_INFINITY, f32::max);
            let max_y = corners
                .iter()
                .map(|p| p.1)
                .fold(f32::NEG_INFINITY, f32::max);
            SnapTarget {
                id: node.id.clone(),
                bounds: SnapRect {
                    x: min_x,
                    y: min_y,
                    width: max_x - min_x,
                    height: max_y - min_y,
                },
            }
        },
    )))
}

fn find<'a>(
    nodes: &'a [SceneNode],
    id: &str,
    ancestors: &mut Vec<&'a SceneNode>,
) -> Option<&'a SceneNode> {
    let mut stack = vec![(nodes, 0usize)];
    while let Some((siblings, next)) = stack.last_mut() {
        if *next == siblings.len() {
            stack.pop();
            ancestors.pop();
            continue;
        }
        let siblings: &'a [SceneNode] = *siblings;
        let node = &siblings[*next];
        *next += 1;
        if node.id == id {
            return Some(node);
        }
        if !node.children.is_empty() {
            ancestors.push(node);
            stack.push((&node.children, 0));
        }
    }
    None
}

fn transform((x, y): (f32, f32), node: &SceneNode) -> (f32, f32) {
    if node.rotation.abs() <= f32::EPSILON && !node.flip_x && !node.flip_y {
        return (x, y);
    }
    let bounds = node.aggregate_bounds();
    let cx = bounds.origin.x + bounds.size.x * 0.5;
    let cy = bounds.origin.y + bounds.size.y * 0.5;
    let (sin, cos) = node.rotation.sin_cos();
    let dx = x - cx;
    let dy = y - cy;
    let rx = dx * cos - dy * sin;
    let ry = dx * sin + dy * cos;
    (
        cx + if node.flip_x { -rx } else { rx },
        cy + if node.flip_y { -ry } else { ry },
    )
}
