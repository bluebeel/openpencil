//! Explicit legacy root -> Page1 derived-route swaps. Normal page table edits
//! retain stable page-id routes and do not clone/rewrite a whole catalog.
use super::ScopedHistoryError;
use crate::{
    component_backing::{resolve_document_location, DocumentNodeLocation},
    EditorState, NodeId,
};
use std::{mem, sync::Arc};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    id: NodeId,
    old: DocumentNodeLocation,
    expected: DocumentNodeLocation,
}
pub(super) fn prepare(state: &EditorState, promoted_id: Option<&str>) -> Vec<Entry> {
    let Some(page) = promoted_id else {
        return Vec::new();
    };
    let page: Arc<str> = Arc::from(page);
    let mut entries = Vec::new();
    for (id, location) in state.components.document_backing().iter() {
        if location.page_index.is_none()
            && location.page_id.is_none()
            && !location.child_path.is_empty()
            && resolve_document_location(&state.doc, location, id.as_str()).is_some()
        {
            entries.push(Entry {
                id: id.clone(),
                old: location.clone(),
                expected: DocumentNodeLocation {
                    page_index: Some(0),
                    page_id: Some(Arc::clone(&page)),
                    child_path: location.child_path.clone(),
                },
            });
        }
    }
    entries.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    entries
}
pub(super) fn validate(state: &EditorState, entries: &[Entry]) -> Result<(), ScopedHistoryError> {
    let backing = state.components.document_backing();
    if entries
        .iter()
        .all(|entry| backing.get(&entry.id) == Some(&entry.expected))
    {
        Ok(())
    } else {
        Err(ScopedHistoryError::NodePathChanged)
    }
}
pub(super) fn install(state: &mut EditorState, entries: &[Entry]) {
    let backing = state.components.document_backing_mut();
    for entry in entries {
        *backing.get_mut(&entry.id).expect("fresh root route") = entry.expected.clone();
    }
}
pub(super) fn exchange(state: &mut EditorState, entries: &mut [Entry]) {
    let backing = state.components.document_backing_mut();
    for entry in entries {
        let live = backing
            .get_mut(&entry.id)
            .expect("prevalidated migration route");
        mem::swap(live, &mut entry.old);
        entry.expected = live.clone();
    }
}
pub(super) fn bytes(entries: &[Entry], capacity: usize) -> usize {
    entries
        .iter()
        .fold(capacity * mem::size_of::<Entry>(), |bytes, entry| {
            bytes
                .saturating_add(entry.id.as_str().len())
                .saturating_add(
                    (entry.old.child_path.len() + entry.expected.child_path.len())
                        * mem::size_of::<usize>(),
                )
                .saturating_add(entry.old.page_id.as_ref().map_or(0, |id| 32 + id.len()))
                .saturating_add(
                    entry
                        .expected
                        .page_id
                        .as_ref()
                        .map_or(0, |id| 32 + id.len()),
                )
        })
}

pub(crate) struct Transition(Vec<Entry>);
impl Transition {
    pub(crate) fn prepare(state: &EditorState, promoted: Option<&str>) -> Self {
        Self(prepare(state, promoted))
    }
    pub(crate) fn install(self, state: &mut EditorState) {
        install(state, &self.0);
    }
}
