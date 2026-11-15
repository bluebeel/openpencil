//! Committed web state apply: moved exact regions and one compact undo entry.
//! Remote changes keep the established globally undoable snapshot-pull contract.

use super::{
    forward_mutation as mutation, forward_regions as regions, patch, ScopedEditorSnapshot,
};
use crate::{
    web_sync::{CommittedFeedError as Error, CommittedMetadata},
    EditorState, History, HistoryEntry,
};
use std::sync::Arc;

/// Historical public name retained for callers; this also covers structural
/// feed data. Held only until repaint/ack succeeds, never a full Doc clone.
pub struct CommittedMetadataApply {
    snapshot: Arc<ScopedEditorSnapshot>,
    history: History,
    generation: u64,
    post_revision: u64,
    history_push_count: u64,
    runtime_high_water: u64,
}

impl EditorState {
    pub fn apply_committed_metadata(
        &mut self,
        batch: CommittedMetadata,
    ) -> Result<Option<CommittedMetadataApply>, Error> {
        let mut scopes = Vec::new();
        let mut refresh_components = false;
        let mut touched = Vec::new();
        for operation in &batch.operations {
            if matches!(
                operation,
                crate::web_sync::committed::Operation::CatalogLabel(_)
            ) {
                continue;
            }
            let scope = mutation::scope(operation.page());
            patch::roots(&self.doc, &scope).map_err(|_| Error::Precondition)?;
            if !scopes.contains(&scope) {
                scopes.push(scope);
            }
            if let crate::web_sync::committed::Operation::Metadata(value) = operation {
                if let Some(id) = crate::NodeId::new_opt(&value.expected_id) {
                    touched.push(id);
                }
                refresh_components |= crate::NodeId::new_opt(&value.expected_id)
                    .is_some_and(|id| self.components.is_document_backed(&id));
            }
        }
        if batch.changes_structure && !refresh_components {
            let backing = self.components.document_backing();
            refresh_components = backing.values().any(|location| {
                let Some(page) = location.page_index_in(&self.doc) else {
                    return true;
                };
                scopes.iter().any(|scope| match (scope, page) {
                    (patch::Scope::Root, None) => true,
                    (patch::Scope::Page { index, .. }, Some(page)) => *index == page,
                    _ => false,
                })
            });
        }
        if batch.operations.is_empty() {
            return Ok(None);
        }
        let mut snapshot =
            ScopedEditorSnapshot::auxiliary_with_components(self, scopes, refresh_components);
        let history = self.history.clone();
        let history_push_count = self.history_push_count;
        let runtime_high_water = self.runtime_id_high_water();
        let mut before: Vec<regions::Before> = Vec::new();
        let mut labels = super::forward_catalog::Labels::default();
        for operation in batch.operations {
            if let crate::web_sync::committed::Operation::CatalogLabel(value) = operation {
                if let Err(error) = labels.apply(self, value) {
                    for old in before.into_iter().rev() {
                        old.restore(&mut self.doc);
                    }
                    labels.restore(self)?;
                    return Err(error);
                }
                continue;
            }
            match mutation::apply(&mut self.doc, operation) {
                Ok(captured) => regions::remember(&mut before, captured),
                Err(error) => {
                    // Every live capture is disjoint, apart from independent
                    // headers/child slots. No unversioned document mutation can
                    // interleave while this exclusive borrow is held.
                    for old in before.into_iter().rev() {
                        old.restore(&mut self.doc);
                    }
                    labels.restore(self)?;
                    return Err(error);
                }
            }
        }
        let label_patches = match labels.compact(self) {
            Ok(patches) => patches,
            Err(_) => {
                for old in before.into_iter().rev() {
                    old.restore(&mut self.doc);
                }
                labels.restore(self)?;
                return Err(Error::Capture);
            }
        };
        regions::normalize(&mut before, &self.doc);
        snapshot.patches = mutation::compact(before, &self.doc);
        snapshot.auxiliary_patches = label_patches;
        if !snapshot.has_canonical_changes() {
            return Ok(None);
        }
        if refresh_components {
            self.components
                .rebuild_from_document_preserving_catalog(&self.doc);
        }
        if batch.changes_structure {
            self.ui.variables.fill_refs.clear();
            self.ui.variables.stroke_refs.clear();
            self.selection = crate::SelectionState::empty();
        } else {
            for id in touched {
                self.ui.variables.fill_refs.remove(&id);
                self.ui.variables.stroke_refs.remove(&id);
            }
        }
        if !snapshot.patches.is_empty() {
            self.editor_ui.clear_document_derived();
            self.codegen.reset_for_document_replacement();
            self.instance_write_virtual_anchor = None;
            jian_ops_schema::image_thumbs::activate_for_document(&self.doc);
        }
        // Do not let selective undo jump across this external state boundary.
        // A stale Full local entry remains behind the remote entry in global
        // LIFO history, or conflicts explicitly for actor-selective traversal.
        snapshot.actor_safe = false;
        let snapshot = Arc::new(snapshot);
        self.history_push_entry(HistoryEntry::Scoped(Arc::clone(&snapshot)));
        self.record_runtime_id_high_water(batch.highest_id);
        Ok(Some(CommittedMetadataApply {
            snapshot,
            history,
            generation: self.document_generation(),
            post_revision: self.document_revision(),
            history_push_count,
            runtime_high_water,
        }))
    }
    pub fn rollback_committed_metadata(
        &mut self,
        applied: CommittedMetadataApply,
    ) -> Result<(), Error> {
        if self.document_generation() != applied.generation
            || self.document_revision() != applied.post_revision
        {
            return Err(Error::Rollback);
        }
        let mut snapshot = (*applied.snapshot).clone();
        snapshot.exchange(self).map_err(|_| Error::Rollback)?;
        self.history = applied.history;
        self.history_push_count = applied.history_push_count;
        self.runtime_node_high_water = applied.runtime_high_water;
        // Keep the monotonic revision counter: a failed paint may already have
        // derived scene data under the rejected revision. Never alias it later.
        if !snapshot.patches.is_empty() {
            self.editor_ui.clear_document_derived();
            self.codegen.reset_for_document_replacement();
        }
        Ok(())
    }
}
