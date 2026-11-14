//! Request-scoped scenes retain a whole layout root and its global definitions.

use jian_ops_schema::node::PenNode;
use jian_scene::layout_scene::{LayoutScene, ScenePage};
use op_editor_core::EditorState;

/// Build the scene needed to export a page or one node.
///
/// Each top-level root is laid out independently. Retaining the entire containing
/// root preserves flex siblings, ancestor transforms, clipping and inherited
/// paint. Components, refs and tokens still resolve against the original document.
/// Unknown IDs (including virtual component children) and workspace presentation
/// use the full builder so those existing export contracts remain unchanged.
pub fn editor_state_to_item_layout_scene(state: &EditorState, item_id: &str) -> LayoutScene {
    crate::selected_scene::item_scene(state, item_id)
}

/// Build only one requested page, or the complete root containing a raw node.
/// Layout still uses the original document's component/token definitions. Unknown
/// and virtual IDs retain the entire requested page; duplicate raw IDs do too.
pub fn editor_state_to_page_item_layout_scene(
    state: &EditorState,
    page_index: usize,
    item_id: Option<&str>,
) -> LayoutScene {
    crate::selected_scene::page_item_scene(state, page_index, item_id)
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

/// Explicit trusted-host counterpart; fresh builders above never use this index.
pub(crate) fn selected_page_roots_cached<'a>(
    state: &'a EditorState,
    page: usize,
    item: Option<&str>,
) -> Option<(ScenePage, &'a [PenNode])> {
    let (meta, roots) = selected_page_roots(state, page, None)?;
    match crate::geometry_cache::values::page_index_cached(state, page) {
        Some(index) => Some((meta, index.select(roots, item))),
        None => selected_page_roots(state, page, item),
    }
}
