//! Request-scoped scenes retain a whole layout root and its global definitions.

use jian_ops_schema::node::PenNode;
use jian_scene::layout_scene::{LayoutScene, ScenePage};
use op_editor_core::{EditorState, NodeId};

/// Build the scene needed to export a page or one node.
///
/// Each top-level root is laid out independently. Retaining the entire containing
/// root preserves flex siblings, ancestor transforms, clipping and inherited
/// paint. Components, refs and tokens still resolve against the original document.
/// Unknown IDs (including virtual component children) and workspace presentation
/// use the full builder so those existing export contracts remain unchanged.
pub fn editor_state_to_item_layout_scene(state: &EditorState, item_id: &str) -> LayoutScene {
    if state.editor_ui.workspace.visible {
        return crate::editor_state_to_layout_scene(state);
    }
    let (mut pages, roots): (Vec<ScenePage>, Vec<&[PenNode]>) =
        if let Some(doc_pages) = &state.doc.pages {
            (
                doc_pages
                    .iter()
                    .map(|page| ScenePage {
                        id: page.id.clone(),
                        name: page.name.clone(),
                        children: Vec::new(),
                    })
                    .collect(),
                doc_pages
                    .iter()
                    .map(|page| page.children.as_slice())
                    .collect(),
            )
        } else {
            let (id, name) = if state.doc.children.is_empty() {
                ("n1", "Page 1")
            } else {
                ("page-1", state.doc.name.as_deref().unwrap_or("Page 1"))
            };
            (
                vec![ScenePage {
                    id: id.into(),
                    name: name.into(),
                    children: Vec::new(),
                }],
                vec![state.doc.children.as_slice()],
            )
        };
    // Page IDs take priority over node IDs, matching export_item's dispatch.
    let selected = pages
        .iter()
        .position(|page| page.id == item_id)
        .map(|index| (index, roots[index]))
        .or_else(|| {
            let id = NodeId::new(item_id);
            roots.iter().enumerate().find_map(|(index, children)| {
                children
                    .iter()
                    .find(|root| {
                        op_editor_core::walkers::find_node(std::slice::from_ref(*root), &id)
                            .is_some()
                    })
                    .map(|root| (index, std::slice::from_ref(root)))
            })
        });
    let Some((page_index, selected_roots)) = selected else {
        return crate::editor_state_to_layout_scene(state);
    };
    pages[page_index].children = crate::editor_scene::roots_to_scene_children(
        state,
        selected_roots,
        &pages[page_index],
        page_index,
        false,
        false,
    );
    LayoutScene {
        active_page_index: state
            .ui
            .active_page_index
            .min(pages.len().saturating_sub(1)),
        pages,
    }
}
