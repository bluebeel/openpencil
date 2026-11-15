//! Exact whole-root pushdown, never a filtered PenDocument. Components,
//! variables, resources and authored metadata remain in the complete borrowed
//! editor. Unknown inter-root dependencies use the existing complete builders.
use jian_ops_schema::{node::PenNode, style::BlendMode};
use jian_scene::layout_scene::{LayoutScene, ScenePage};
use op_editor_core::{EditorState, PenNodeExt};
use std::collections::{HashMap, HashSet};

fn pages(state: &EditorState) -> Vec<ScenePage> {
    match &state.doc.pages {
        Some(pages) => pages
            .iter()
            .map(|page| ScenePage {
                id: page.id.clone(),
                name: page.name.clone(),
                children: Vec::new(),
            })
            .collect(),
        None => vec![ScenePage {
            id: if state.doc.children.is_empty() {
                "n1"
            } else {
                "page-1"
            }
            .into(),
            name: if state.doc.children.is_empty() {
                "Page 1"
            } else {
                state.doc.name.as_deref().unwrap_or("Page 1")
            }
            .into(),
            children: Vec::new(),
        }],
    }
}
fn page_roots(state: &EditorState, page: usize) -> Option<&[PenNode]> {
    match &state.doc.pages {
        Some(pages) => pages.get(page).map(|page| page.children.as_slice()),
        None => (page == 0).then_some(state.doc.children.as_slice()),
    }
}
fn active_index(state: &EditorState) -> usize {
    state.ui.active_page_index.min(
        state
            .doc
            .pages
            .as_ref()
            .map_or(0, |pages| pages.len().saturating_sub(1)),
    )
}

/// Borrowed proof also checks IDs unrelated to the requested targets: the
/// adapter's page-wide rectangle map has last-ID geometry semantics, so any
/// duplicate ID makes independent conversion unsafe. A single complete read
/// is much cheaper than cloning/resolving/laying out every unrelated root.
fn independent_roots(roots: &[PenNode], ids: &[String]) -> Option<Vec<usize>> {
    if ids.is_empty() || ids.iter().any(|id| id.is_empty() || id.contains("__")) {
        return None;
    }
    let requested: HashSet<_> = ids.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    let mut found = HashMap::new();
    for (index, root) in roots.iter().enumerate() {
        if root.base().mask_type.is_some()
            || root
                .base()
                .blend_mode
                .as_ref()
                .is_some_and(|mode| *mode != BlendMode::Normal)
        {
            // Preserve sibling-mask sequences and complete backdrop context.
            return None;
        }
        let mut pending = vec![std::slice::from_ref(root).iter()];
        while let Some(siblings) = pending.last_mut() {
            if let Some(node) = siblings.next() {
                let id = node.id_str();
                if !seen.insert(id)
                    || matches!(node, PenNode::Ref(_))
                    || node.base().explain.as_deref().is_some_and(|explain| {
                        explain.starts_with(op_editor_core::table_filter_contract::TABLE_MARKER)
                    })
                {
                    // Expanded virtual IDs and generated table bindings have
                    // broader dependencies; retain their complete old path.
                    return None;
                }
                if requested.contains(id) {
                    found.insert(id, index);
                }
                if let Some(children) = node.children() {
                    pending.push(children.iter());
                }
            } else {
                pending.pop();
            }
        }
    }
    if found.len() != requested.len() {
        return None;
    }
    let mut selected: Vec<_> = found.into_values().collect();
    selected.sort_unstable();
    selected.dedup();
    (selected.len() < roots.len()).then_some(selected)
}

pub(crate) fn active_can_push_down(state: &EditorState, ids: &[String]) -> bool {
    if state.editor_ui.workspace.visible {
        return false;
    }
    let page = active_index(state);
    let Some(roots) = page_roots(state, page) else {
        return false;
    };
    match crate::geometry_cache::values::page_index_cached(state, page) {
        Some(index) => index.independent_roots(roots.len(), ids).is_some(),
        None => independent_roots(roots, ids).is_some(),
    }
}

fn build_page(state: &EditorState, page_index: usize, ids: &[String]) -> LayoutScene {
    let mut pages = pages(state);
    let Some(page) = pages.get(page_index) else {
        return crate::editor_state_to_layout_scene(state);
    };
    let Some(roots) = page_roots(state, page_index) else {
        return crate::editor_state_to_layout_scene(state);
    };
    let selected = independent_roots(roots, ids);
    let children =
        match selected {
            Some(selected) => {
                // One exact global variable table; disjoint borrowed root ranges
                // stream through preparation/payload/scene without cloning roots
                // merely to form a sparse input list. Every containing root keeps
                // all its flex siblings, clips, masks and ancestor paint.
                let variables = crate::editor_state_var_table(state);
                let mut children = Vec::with_capacity(selected.len());
                let mut at = 0;
                while at < selected.len() {
                    let start = selected[at];
                    let mut end = start + 1;
                    at += 1;
                    while at < selected.len() && selected[at] == end {
                        end += 1;
                        at += 1;
                    }
                    let payload = crate::editor_scene::roots_to_page_payload(
                        state,
                        &roots[start..end],
                        page,
                        page_index,
                        false,
                        false,
                    );
                    children.extend(payload.children.into_iter().map(|node| {
                        crate::layout_scene::node_payload_to_scene(node, &variables, 1.0)
                    }));
                }
                children
            }
            None => crate::editor_scene::roots_to_scene_children(
                state, roots, page, page_index, false, false,
            ),
        };
    pages[page_index].children = children;
    LayoutScene {
        active_page_index: state
            .ui
            .active_page_index
            .min(pages.len().saturating_sub(1)),
        pages,
    }
}

/// Explicit request-specific counterpart. Arbitrary-argument public tool
/// snapshots retain their existing complete scenes. Empty/unknown/ambiguous
/// requests and presentation work retain complete active-page behavior.
pub fn editor_state_to_active_items_layout_scene(
    state: &EditorState,
    ids: &[String],
) -> LayoutScene {
    if state.editor_ui.workspace.visible {
        return crate::editor_state_to_active_page_layout_scene(state);
    }
    build_page(state, active_index(state), ids)
}

pub(crate) fn page_item_scene(state: &EditorState, page: usize, item: Option<&str>) -> LayoutScene {
    if state.editor_ui.workspace.visible {
        return crate::editor_state_to_layout_scene(state);
    }
    let ids: Vec<_> = item.into_iter().map(str::to_owned).collect();
    build_page(state, page, &ids)
}

pub(crate) fn item_scene(state: &EditorState, item: &str) -> LayoutScene {
    if state.editor_ui.workspace.visible {
        return crate::editor_state_to_layout_scene(state);
    }
    let metadata = pages(state);
    // Page IDs take priority over node IDs, as in export_item dispatch.
    if let Some(page) = metadata.iter().position(|page| page.id == item) {
        return build_page(state, page, &[]);
    }
    if item.is_empty() || item.contains("__") {
        return crate::editor_state_to_layout_scene(state);
    }
    let mut found = None;
    for page in 0..metadata.len() {
        let Some(roots) = page_roots(state, page) else {
            continue;
        };
        let mut pending = vec![roots.iter()];
        while let Some(siblings) = pending.last_mut() {
            if let Some(node) = siblings.next() {
                if node.id_str() == item {
                    if found.replace(page).is_some() {
                        return crate::editor_state_to_layout_scene(state);
                    }
                }
                if let Some(children) = node.children() {
                    pending.push(children.iter());
                }
            } else {
                pending.pop();
            }
        }
    }
    match found {
        Some(page) => build_page(state, page, &[item.to_owned()]),
        None => crate::editor_state_to_layout_scene(state),
    }
}
