//! Bound paint allocation while retaining the complete authoritative layout solve.
use super::{ComputedGeometry, EditorState};
use jian_ops_schema::node::PenNode;
use op_editor_core::pen_node_ext::{clone_without_children, PenNodeExt};
use std::collections::HashSet;

const MAX_TARGET_NODES: usize = 1024;

fn path_to<'a>(node: &'a PenNode, target: &str, path: &mut Vec<&'a PenNode>) -> bool {
    path.push(node);
    if node.id_str() == target {
        return true;
    }
    if let Some(children) = node.children() {
        for child in children {
            if path_to(child, target, path) {
                return true;
            }
        }
    }
    path.pop();
    false
}

fn unique<'a>(nodes: &'a [PenNode], ids: &mut HashSet<&'a str>) -> bool {
    nodes.iter().all(|node| {
        ids.insert(node.id_str()) && node.children().is_none_or(|children| unique(children, ids))
    })
}

fn bounded_size(node: &PenNode, count: &mut usize) -> bool {
    *count += 1;
    *count <= MAX_TARGET_NODES
        && node
            .children()
            .is_none_or(|children| children.iter().all(|child| bounded_size(child, count)))
}

pub(super) fn try_geometry(
    state: &EditorState,
    page: usize,
    parent: &str,
) -> Option<Vec<ComputedGeometry>> {
    if state.editor_ui.workspace.visible || parent.contains("__") {
        return None;
    }
    let raw = match &state.doc.pages {
        Some(pages) => pages.get(page)?.children.as_slice(),
        None if page == 0 => state.doc.children.as_slice(),
        None => return None,
    };
    // Duplicate IDs have shared layout lookup and last-raw-metadata semantics.
    // Preserve that complete path instead of guessing which duplicate survives.
    let mut ids = HashSet::new();
    if !unique(raw, &mut ids) || !ids.contains(parent) {
        return None;
    }
    drop(ids); // Do not keep the full raw-ID inventory during layout.
    let (page_meta, roots) = crate::item_scene::selected_page_roots(state, page, Some(parent))?;
    if roots.len() != 1 {
        return None;
    }
    let prepared = crate::editor_scene::prepared_page_roots(state, roots, &page_meta, false, false);
    let [root] = prepared.as_ref() else {
        return None;
    };
    if matches!(&prepared, std::borrow::Cow::Owned(_))
        && !unique(std::slice::from_ref(root), &mut HashSet::new())
    {
        return None;
    }
    let mut path = Vec::new();
    if !path_to(root, parent, &mut path) || path.len() > 128 {
        return None; // Unknown/virtual IDs retain the old page/scene fallback.
    }
    let target = *path.last()?;
    let mut target_nodes = 0;
    if !bounded_size(target, &mut target_nodes) {
        return None; // Do not add a large owned target clone to the legacy path.
    }
    // These ancestors affect child payload geometry only through the full rect
    // map and the ordinary text-center context. Widgets/ref ancestors may have
    // index-sensitive child transforms, so keep the complete existing builder.
    if path[..path.len() - 1].iter().any(|node| {
        !matches!(
            node,
            PenNode::Frame(_) | PenNode::Group(_) | PenNode::Rectangle(_)
        )
    }) {
        return None;
    }
    let mut spine = target.clone();
    for ancestor in path[..path.len() - 1].iter().rev() {
        let mut metadata = clone_without_children(ancestor);
        metadata.children_mut()?.push(spine);
        spine = metadata;
    }
    let payload = crate::adapter::pages::bounded_root_payload(
        root,
        &spine,
        state.editor_ui.preserve_authored_geometry,
    );
    Some(vec![ComputedGeometry::from_payload(payload)])
}

#[cfg(test)]
#[path = "bounded_geometry_tests.rs"]
mod tests;
