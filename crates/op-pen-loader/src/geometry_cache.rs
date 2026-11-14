//! Explicit trusted-host reuse of computed geometry; public fresh builders remain fresh.
use crate::{computed_geometry::ComputedGeometry, editor_state_to_page_item_geometry};
use op_editor_core::EditorState;
use std::{
    collections::BTreeMap,
    mem::size_of,
    sync::{Arc, Mutex, OnceLock, Weak},
};

const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 64;

struct Key {
    owner: Weak<()>,
    generation: u64,
    revision: u64,
    page: usize,
    scope: (usize, usize),
    parent_query: Option<String>,
    root_rectangles: bool,
    preserve: bool,
    workspace: bool,
    preview: bool,
    active_page: usize,
    theme: BTreeMap<String, String>,
    font_generation: u64,
    components: Vec<(String, usize)>,
}
impl Key {
    fn new(
        state: &EditorState,
        page: usize,
        item: Option<&str>,
        font_generation: u64,
    ) -> Option<Self> {
        let roots = if state.editor_ui.workspace.visible {
            crate::item_scene::selected_page_roots(state, page, None)?.1
        } else {
            crate::item_scene::selected_page_roots(state, page, item)?.1
        };
        let mut components = Vec::new();
        if state.document_revision() == 0 {
            // External prototypes have no revision contract. Keep them fresh;
            // document-backed prototypes are identified by their exact source.
            for c in &state.components.components {
                if !state.components.is_document_backed(&c.id) {
                    return None;
                }
                let root = state.components.resolved_root(&state.doc, &c.id)?;
                components.push((c.id.as_str().to_owned(), root as *const _ as usize));
            }
        }
        Some(Self {
            owner: state.derived_cache_owner(),
            generation: state.document_generation(),
            revision: state.document_revision(),
            page,
            scope: (roots.as_ptr() as usize, roots.len()),
            parent_query: None,
            root_rectangles: false,
            preserve: state.editor_ui.preserve_authored_geometry,
            workspace: state.editor_ui.workspace.visible,
            preview: state.editor_ui.preview.mode,
            active_page: state.ui.active_page_index,
            theme: state.ui.variables.active_theme.clone(),
            font_generation,
            components,
        })
    }
    fn matches(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.owner, &other.owner)
            && self.generation == other.generation
            && self.revision == other.revision
            && self.page == other.page
            && self.scope == other.scope
            && self.parent_query == other.parent_query
            && self.root_rectangles == other.root_rectangles
            && self.preserve == other.preserve
            && self.workspace == other.workspace
            && self.preview == other.preview
            && self.active_page == other.active_page
            && self.theme == other.theme
            && self.font_generation == other.font_generation
            && self.components == other.components
    }
    fn bytes(&self) -> usize {
        size_of::<Self>() + 32 + self.parent_query.as_ref().map_or(0, String::capacity)
            + self.components.capacity() * size_of::<(String,usize)>()
            + self.components.iter().map(|(id,_)| id.capacity()).sum::<usize>()
            // Conservatively charge BTree node capacity, including sparse leaves.
            + self.theme.iter().map(|(k,v)| 1024+k.capacity()+v.capacity()).sum::<usize>()
    }
}
#[path = "geometry_cache_values.rs"]
pub(crate) mod values;
use values::{get_or_compute, Store};

fn shared_store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Store::new()))
}
fn geometry_bytes(nodes: &Vec<ComputedGeometry>) -> usize {
    nodes.capacity() * size_of::<ComputedGeometry>()
        + nodes
            .iter()
            .map(|node| {
                node.id.capacity()
                    + match &node.kind {
                        jian_scene::layout_scene::NodeKind::Other(s) => s.capacity(),
                        _ => 0,
                    }
                    + geometry_bytes(&node.children)
            })
            .sum::<usize>()
}
/// Opt-in only for hosts that mark every canonical edit/reload with the editor's
/// generation/revision contract. Direct-field-mutating callers use the fresh API.
pub fn editor_state_to_page_item_geometry_cached(
    state: &EditorState,
    page: usize,
    item: Option<&str>,
) -> Arc<Vec<ComputedGeometry>> {
    get_or_compute(
        shared_store(),
        Key::new(state, page, item, crate::current_font_generation()),
        || editor_state_to_page_item_geometry(state, page, item),
    )
}

#[cfg(test)]
#[path = "geometry_cache_tests.rs"]
mod tests;

/// Separate cache identity: a parent-only spine must never satisfy a full-root query.
pub fn editor_state_to_page_parent_geometry_cached(
    state: &EditorState,
    page: usize,
    parent: &str,
) -> Arc<Vec<ComputedGeometry>> {
    let key = (parent.len() <= 4096)
        .then(|| Key::new(state, page, Some(parent), crate::current_font_generation()))
        .flatten()
        .map(|mut key| {
            key.parent_query = Some(parent.to_owned());
            key
        });
    get_or_compute(shared_store(), key, || {
        crate::computed_geometry::editor_state_to_page_parent_geometry_with_rect_reuse(
            state, page, parent,
        )
    })
}
