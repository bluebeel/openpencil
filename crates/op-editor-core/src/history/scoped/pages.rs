//! Page-list deltas retain only removed pages, stable order witnesses, and an
//! explicit reversible legacy-root migration. Other pages remain live owners.
use super::{splice_order::Anchors, Scope, ScopedEditorSnapshot, ScopedHistoryError};
use crate::{EditorState, SharedNode};
use jian_ops_schema::PenDocument;
use std::{mem, sync::Arc};
mod cohort;
mod collision;
mod command;
mod exchange;
mod forecast;
mod routes;
mod shared;
pub(super) use exchange::Prepared;
pub(super) use shared::SharedPage;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Migration {
    pub page: String,
    pub header: jian_ops_schema::page::PenPage,
    pub legacy_present: bool,
    // True while the migrated forest lives in its explicit page.
    pub promoted: bool,
    pub routes: Vec<routes::Entry>,
}
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Kind {
    Splice {
        expected: Vec<String>,
        old: Vec<Arc<SharedPage>>,
    },
    Move {
        id: String,
    },
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Patch {
    pub kind: Kind,
    pub placement: Anchors,
    pub recent: Vec<Anchors>,
    pub affected: Arc<Vec<String>>,
    pub component_ids: Arc<Vec<String>>,
    pub references: Arc<Vec<String>>,
    pub migration: Option<Migration>,
    pub external: std::collections::HashMap<String, usize>,
}
impl Patch {
    pub(super) fn retained_roots(&self) -> impl Iterator<Item = &Arc<SharedNode>> {
        let pages: &[Arc<SharedPage>] = match &self.kind {
            Kind::Splice { old, .. } => old,
            _ => &[],
        };
        pages.iter().flat_map(|page| page.children.iter())
    }
    pub(super) fn storage_bytes(&self) -> usize {
        let anchors = |anchor: &Anchors| {
            anchor.ids.capacity() * mem::size_of::<String>()
                + anchor.ids.iter().map(String::capacity).sum::<usize>()
                + anchor.left.as_ref().map_or(0, String::capacity)
                + anchor.right.as_ref().map_or(0, String::capacity)
        };
        let mut bytes = self.external.capacity() * (mem::size_of::<(String, usize)>() + 32)
            + self.external.keys().map(String::capacity).sum::<usize>()
            + mem::size_of::<Self>()
            + anchors(&self.placement)
            + self.recent.capacity() * mem::size_of::<Anchors>()
            + self.recent.iter().map(anchors).sum::<usize>()
            + self.affected.capacity() * mem::size_of::<String>()
            + self.affected.iter().map(String::capacity).sum::<usize>();
        for ids in [&self.component_ids, &self.references] {
            bytes = bytes
                .saturating_add(ids.capacity() * mem::size_of::<String>())
                .saturating_add(ids.iter().map(String::capacity).sum::<usize>());
        }
        if let Kind::Splice { expected, old } = &self.kind {
            bytes = bytes.saturating_add(
                expected.capacity() * mem::size_of::<String>()
                    + expected.iter().map(String::capacity).sum::<usize>()
                    + old.capacity() * mem::size_of::<Arc<SharedPage>>()
                    + old.iter().map(|page| page.bytes()).sum::<usize>(),
            );
        }
        if let Some(migration) = &self.migration {
            bytes = bytes
                .saturating_add(routes::bytes(
                    &migration.routes,
                    migration.routes.capacity(),
                ))
                .saturating_add(
                    migration.page.capacity()
                        + crate::history_snapshot::command_capture_value_bytes(
                            &migration.header,
                            usize::MAX / 4,
                        )
                        .unwrap_or(usize::MAX / 4),
                );
        }
        bytes
    }
    fn changes_page(&self, id: &str) -> bool {
        !matches!(self.kind, Kind::Move { .. }) && self.affected.iter().any(|page| page == id)
    }
    pub(super) fn conflicts(&self, other: &ScopedEditorSnapshot) -> bool {
        if let Some(page) = &other.page_patch {
            if self.migration.is_some() || page.migration.is_some() {
                return true;
            }
            if self.affected.iter().any(|id| page.affected.contains(id)) {
                return true;
            }
        }
        for patch in &other.patches {
            for scope in patch
                .regions()
                .into_iter()
                .flatten()
                .map(|region| region.scope)
                .chain(patch.splices().map(|splice| &splice.scope))
            {
                if (self.migration.is_some() && matches!(scope, Scope::Root))
                    || matches!(scope, Scope::Page { id, .. } if self.changes_page(id))
                {
                    return true;
                }
            }
        }
        let removes_definitions = !self.component_ids.is_empty()
            && matches!(&self.kind, Kind::Splice { expected, .. } if !expected.is_empty());
        if removes_definitions {
            if let Some(page) = &other.page_patch {
                if page
                    .references
                    .iter()
                    .any(|id| self.component_ids.contains(id))
                {
                    return true;
                }
            } else if !other.patches.is_empty() {
                // Older node history has no exact read-dependency receipt.
                // Conservatively conflict instead of breaking a foreign Ref;
                // scalar/key-only entries and disjoint page edits stay usable.
                return true;
            }
        }
        other.auxiliary_patches.iter().any(|patch| match patch {
            super::auxiliary::Patch::Page { id, .. } => self.changes_page(id),
            _ => false,
        })
    }
    pub(super) fn restores(&self, id: &str) -> bool {
        matches!(&self.kind, Kind::Splice { old, .. } if old.iter().any(|page| page.id() == id))
            || self
                .migration
                .as_ref()
                .is_some_and(|migration| !migration.promoted && migration.page == id)
    }
}
#[derive(Debug)]
pub(super) struct Active {
    index: usize,
    id: Option<String>,
}
impl Active {
    pub(super) fn capture(state: &EditorState) -> Self {
        Self {
            index: state.ui.active_page_index,
            id: active_id(state),
        }
    }
}
pub(super) fn active_id(state: &EditorState) -> Option<String> {
    state
        .doc
        .pages
        .as_ref()?
        .get(state.ui.active_page_index)
        .map(|page| page.id.clone())
}
pub(super) fn unique_index(doc: &PenDocument, id: &str) -> Option<usize> {
    let mut found = doc
        .pages
        .as_ref()?
        .iter()
        .enumerate()
        .filter(|(_, page)| page.id == id);
    let index = found.next()?.0;
    found.next().is_none().then_some(index)
}
pub(super) fn index_hint(doc: &PenDocument, index: usize, id: &str) -> Option<usize> {
    if doc
        .pages
        .as_ref()?
        .get(index)
        .is_some_and(|page| page.id == id)
    {
        Some(index)
    } else {
        unique_index(doc, id)
    }
}
pub(super) fn exchange_active(
    snapshot: &mut ScopedEditorSnapshot,
    state: &mut EditorState,
    before: Active,
) {
    state.ui.active_page_index = snapshot
        .active_page_id
        .as_deref()
        .and_then(|id| index_hint(&state.doc, snapshot.active_page_index, id))
        .unwrap_or(snapshot.active_page_index);
    snapshot.active_page_index = before.index;
    snapshot.active_page_id = before.id;
    snapshot.page_count = state.doc.pages.as_ref().map(Vec::len);
}
pub(super) fn validate_actor_shape(
    snapshot: &ScopedEditorSnapshot,
    state: &EditorState,
) -> Result<(), ScopedHistoryError> {
    if state.document_generation() != snapshot.generation {
        return Err(ScopedHistoryError::DocumentGenerationChanged {
            expected: snapshot.generation,
            actual: state.document_generation(),
        });
    }
    for scope in &snapshot.scopes {
        if let Scope::Page { index, id } = scope {
            index_hint(&state.doc, *index, id).ok_or(ScopedHistoryError::PageShapeChanged)?;
        }
        super::patch::roots(&state.doc, scope)?;
    }
    if let Some(id) = &snapshot.active_page_id {
        if index_hint(&state.doc, snapshot.active_page_index, id).is_none()
            && !snapshot
                .page_patch
                .as_ref()
                .is_some_and(|page| page.restores(id))
        {
            return Err(ScopedHistoryError::PageShapeChanged);
        }
    } else if snapshot.page_count != state.doc.pages.as_ref().map(Vec::len)
        && snapshot.page_patch.is_none()
    {
        return Err(ScopedHistoryError::PageShapeChanged);
    }
    Ok(())
}
