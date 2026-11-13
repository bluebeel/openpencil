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

/// Build only one requested page, or the complete root containing a raw node.
/// Layout still uses the original document's component/token definitions. Unknown
/// and virtual IDs retain the entire requested page; duplicate raw IDs do too.
pub fn editor_state_to_page_item_layout_scene(
    state: &EditorState,
    page_index: usize,
    item_id: Option<&str>,
) -> LayoutScene {
    if state.editor_ui.workspace.visible {
        return crate::editor_state_to_layout_scene(state);
    }
    let mut pages: Vec<ScenePage> = match &state.doc.pages {
        Some(pages) if !pages.is_empty() => pages
            .iter()
            .map(|page| ScenePage {
                id: page.id.clone(),
                name: page.name.clone(),
                children: Vec::new(),
            })
            .collect(),
        Some(_) => return crate::editor_state_to_layout_scene(state),
        None => vec![ScenePage {
            id: if state.doc.children.is_empty() {
                "n1"
            } else {
                "page-1"
            }
            .into(),
            name: state.doc.name.as_deref().unwrap_or("Page 1").into(),
            children: Vec::new(),
        }],
    };
    let roots = match &state.doc.pages {
        Some(doc_pages) => doc_pages
            .get(page_index)
            .map(|page| page.children.as_slice()),
        None => (page_index == 0).then_some(state.doc.children.as_slice()),
    };
    let Some(roots) = roots else {
        return crate::editor_state_to_layout_scene(state);
    };
    let selected = selected_page_roots(state, page_index, item_id)
        .map(|(_, roots)| roots)
        .unwrap_or(roots);
    pages[page_index].children = crate::editor_scene::roots_to_scene_children(
        state,
        selected,
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

pub(crate) fn selected_page_roots<'a>(
    state: &'a EditorState,
    page_index: usize,
    item_id: Option<&str>,
) -> Option<(ScenePage, &'a [PenNode])> {
    let (id, name, roots) = match &state.doc.pages {
        Some(pages) => {
            let page = pages.get(page_index)?;
            (
                page.id.as_str(),
                page.name.as_str(),
                page.children.as_slice(),
            )
        }
        None if page_index == 0 => (
            if state.doc.children.is_empty() {
                "n1"
            } else {
                "page-1"
            },
            state.doc.name.as_deref().unwrap_or("Page 1"),
            state.doc.children.as_slice(),
        ),
        None => return None,
    };
    fn count(nodes: &[PenNode], id: &str) -> usize {
        use op_editor_core::pen_node_ext::PenNodeExt;
        nodes
            .iter()
            .map(|node| {
                usize::from(node.id_str() == id)
                    + node.children().map_or(0, |children| count(children, id))
            })
            .sum()
    }
    let selected = item_id
        .filter(|id| !id.contains("__"))
        .and_then(|id| {
            let mut found = None;
            let mut matches = 0;
            for root in roots {
                let hits = count(std::slice::from_ref(root), id);
                if hits > 0 {
                    found = Some(root);
                    matches += hits;
                }
                if matches > 1 {
                    return None;
                }
            }
            found.map(std::slice::from_ref)
        })
        .unwrap_or(roots);
    Some((
        ScenePage {
            id: id.into(),
            name: name.into(),
            children: Vec::new(),
        },
        selected,
    ))
}
