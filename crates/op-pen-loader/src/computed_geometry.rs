//! Compact query geometry produced by the canonical font/flex/payload pipeline.
use jian_scene::layout_scene::{NodeKind, SceneNode};
use op_editor_core::{render_backend::Rect, EditorState};

/// Geometry-only counterpart of a paint scene. Bounds obey SceneNode's exact
/// aggregate rule; paint/text/style payloads are released after canonical layout.
#[derive(Debug)]
pub struct ComputedGeometry {
    pub id: String,
    pub kind: NodeKind,
    pub children: Vec<ComputedGeometry>,
    bounds: Rect,
    visual_bounds: Rect,
}

impl ComputedGeometry {
    pub fn aggregate_bounds(&self) -> Rect {
        self.bounds
    }
    pub fn visual_bounds(&self) -> Rect {
        self.visual_bounds
    }

    pub fn find(&self, id: &str) -> Option<&Self> {
        if self.id == id {
            Some(self)
        } else {
            self.children.iter().find_map(|child| child.find(id))
        }
    }

    fn from_payload(mut node: crate::payload::NodePayload) -> Self {
        let id = std::mem::take(&mut node.id);
        let kind = crate::layout_scene::str_to_kind(&node.kind);
        let own_bounds = Rect::xywh(node.x, node.y, node.w, node.h);
        let hidden = node.hidden;
        let clip = node.clip_content;
        let children = std::mem::take(&mut node.children);
        drop(node);
        let children: Vec<_> = children.into_iter().map(Self::from_payload).collect();
        let bounds = SceneNode::compute_aggregate_bounds_from_rects(
            own_bounds,
            children.iter().map(Self::aggregate_bounds),
        );
        let visual_bounds = SceneNode::compute_visual_bounds(
            own_bounds,
            hidden,
            clip,
            children.iter().map(Self::visual_bounds),
        );
        Self {
            id,
            kind,
            children,
            bounds,
            visual_bounds,
        }
    }

    fn from_scene(mut node: SceneNode) -> Self {
        let own = node.bounds;
        let hidden = node.hidden;
        let clip = node.clip_content;
        let id = std::mem::take(&mut node.id);
        let kind = std::mem::replace(&mut node.kind, NodeKind::Group);
        let children = std::mem::take(&mut node.children);
        drop(node);
        let children: Vec<_> = children.into_iter().map(Self::from_scene).collect();
        let bounds = SceneNode::compute_aggregate_bounds_from_rects(
            own,
            children.iter().map(Self::aggregate_bounds),
        );
        let visual_bounds = SceneNode::compute_visual_bounds(
            own,
            hidden,
            clip,
            children.iter().map(Self::visual_bounds),
        );
        Self {
            id,
            kind,
            children,
            bounds,
            visual_bounds,
        }
    }
}

/// Resolve complete containing roots using the same layout, component, token,
/// widget and legacy geometry rules as the renderer, without retaining paints.
pub fn editor_state_to_page_item_geometry(
    state: &EditorState,
    page_index: usize,
    item_id: Option<&str>,
) -> Vec<ComputedGeometry> {
    if !state.editor_ui.workspace.visible {
        if let Some((page, roots)) =
            crate::item_scene::selected_page_roots(state, page_index, item_id)
        {
            let payload = crate::editor_scene::roots_to_page_payload(
                state, roots, &page, page_index, false, false,
            );
            return payload
                .children
                .into_iter()
                .map(ComputedGeometry::from_payload)
                .collect();
        }
    }
    crate::editor_state_to_layout_scene(state)
        .pages
        .into_iter()
        .nth(page_index)
        .into_iter()
        .flat_map(|page| page.children)
        .map(ComputedGeometry::from_scene)
        .collect()
}

#[cfg(test)]
#[path = "computed_geometry_tests.rs"]
mod tests;
