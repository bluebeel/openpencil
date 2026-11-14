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
struct Entry {
    key: Key,
    value: Arc<Vec<ComputedGeometry>>,
    bytes: usize,
}
struct Store {
    entries: Vec<Entry>,
    bytes: usize,
}
impl Store {
    fn new() -> Self {
        Self {
            entries: Vec::with_capacity(MAX_ENTRIES),
            bytes: 0,
        }
    }
    fn base_bytes(&self) -> usize {
        size_of::<Self>() + self.entries.capacity() * size_of::<Entry>()
    }
    fn take_dead(&mut self, dead: &mut Vec<Entry>) {
        let mut i = 0;
        while i < self.entries.len() {
            if self.entries[i].key.owner.strong_count() == 0 {
                let old = self.entries.remove(i);
                self.bytes -= old.bytes;
                dead.push(old);
            } else {
                i += 1;
            }
        }
    }
    fn hit(&mut self, key: &Key) -> Option<Arc<Vec<ComputedGeometry>>> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.key.matches(key))?;
        let entry = self.entries.remove(index);
        let value = Arc::clone(&entry.value);
        self.entries.push(entry);
        Some(value)
    }
}
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
fn get_or_compute(
    store: &Mutex<Store>,
    key: Option<Key>,
    compute: impl FnOnce() -> Vec<ComputedGeometry>,
) -> Arc<Vec<ComputedGeometry>> {
    let Some(key) = key else {
        return Arc::new(compute());
    };
    let mut dead = Vec::new();
    let hit = {
        let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
        store.take_dead(&mut dead);
        store.hit(&key)
    };
    drop(dead);
    if let Some(value) = hit {
        return value;
    }
    // No cache/writer lock crosses layout, font measurement or payload conversion.
    let value = Arc::new(compute());
    let bytes = 64usize
        .saturating_add(key.bytes())
        .saturating_add(geometry_bytes(&value));
    let mut evicted = Vec::new();
    let result = {
        let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = store.hit(&key) {
            existing
        } else if bytes.saturating_add(store.base_bytes()) > MAX_BYTES {
            Arc::clone(&value)
        } else {
            while store.entries.len() >= MAX_ENTRIES
                || store.bytes + bytes + store.base_bytes() > MAX_BYTES
            {
                let old = store.entries.remove(0);
                store.bytes -= old.bytes;
                evicted.push(old);
            }
            store.bytes += bytes;
            store.entries.push(Entry {
                key,
                value: Arc::clone(&value),
                bytes,
            });
            Arc::clone(&value)
        }
    };
    drop(evicted);
    result
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
        crate::computed_geometry::editor_state_to_page_parent_geometry(state, page, parent)
    })
}
