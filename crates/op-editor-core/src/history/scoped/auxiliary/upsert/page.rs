//! A new Components page is a normal ordered page-list inverse. Subsequent
//! foreign edits to that page therefore conflict instead of being discarded.
use super::*;
use crate::history::scoped::{pages, splice_order::Anchors, ScopedEditorSnapshot};
use std::collections::HashMap;
pub(super) fn prepare(
    pending: &Pending,
    state: &EditorState,
    snapshot: &mut ScopedEditorSnapshot,
) -> Result<(), ScopedHistoryError> {
    let UpsertDestination::NewPage { before, id } = &pending.prepared.destination else {
        return Ok(());
    };
    let current = state
        .doc
        .pages
        .as_ref()
        .ok_or(ScopedHistoryError::PageShapeChanged)?;
    if current.iter().map(|page| &page.id).eq(before) {
        return Ok(());
    }
    if current.len() != before.len() + 1
        || !current
            .iter()
            .take(before.len())
            .map(|page| &page.id)
            .eq(before)
    {
        return Err(ScopedHistoryError::PageShapeChanged);
    }
    let page = current.last().unwrap();
    if page.id != *id
        || page.name != crate::page_mutators::COMPONENTS_PAGE_NAME
        || page.children.len() != 1
        || !pending.expected.equals_node(&page.children[0])
    {
        return Err(ScopedHistoryError::PageShapeChanged);
    }
    snapshot.page_count = Some(current.len());
    snapshot.page_patch = Some(pages::Patch {
        kind: pages::Kind::Splice {
            expected: vec![id.clone()],
            old: Vec::new(),
        },
        placement: Anchors {
            ids: vec![id.clone()],
            left: before.last().cloned(),
            right: None,
            required: false,
        },
        recent: Vec::new(),
        affected: Arc::new(vec![id.clone()]),
        component_ids: Arc::new(vec![pending.prepared.id.as_str().into()]),
        references: Arc::new(Vec::new()),
        migration: None,
        external: HashMap::new(),
    });
    Ok(())
}
