//! Web metadata feed apply: exact target-header undo, not a full Doc snapshot.

use super::{patch, PendingScopedSnapshot, ScopedEditorSnapshot, TargetSpec};
use crate::{
    web_sync::{CommittedFeedError as Error, CommittedMetadata},
    EditorState, History, HistoryEntry, PenNodeExt,
};
use std::sync::Arc;

/// Kept only until the associated repaint succeeds; lets a failed paint retain
/// the old cursor and roll back exactly without a hidden full-document copy.
pub struct CommittedMetadataApply {
    snapshot: Arc<ScopedEditorSnapshot>,
    history: History,
    generation: u64,
    post_revision: u64,
    history_push_count: u64,
}

impl EditorState {
    pub fn apply_committed_metadata(
        &mut self,
        batch: CommittedMetadata,
    ) -> Result<Option<CommittedMetadataApply>, Error> {
        let mut scopes = Vec::new();
        let mut targets = Vec::new();
        let mut refresh_components = false;
        let mut touched = Vec::new();
        for operation in &batch.operations {
            let scope = match &operation.page {
                None => patch::Scope::Root,
                Some(page) => patch::Scope::Page {
                    index: page.index,
                    id: page.id.clone(),
                },
            };
            let current = patch::node(
                patch::roots(&self.doc, &scope).map_err(|_| Error::Precondition)?,
                &operation.path,
            )
            .ok_or(Error::Precondition)?;
            if current.id_str() != operation.expected_id
                || operation.value.id_str() != operation.expected_id
                || std::mem::discriminant(current) != std::mem::discriminant(&operation.value)
                || current.children().is_some() != operation.children_present
                || operation.value.children().is_some() != operation.children_present
                || operation
                    .value
                    .children()
                    .is_some_and(|children| !children.is_empty())
            {
                return Err(Error::Precondition);
            }
            if let Some(id) = crate::NodeId::new_opt(&operation.expected_id) {
                refresh_components |= self.components.is_document_backed(&id);
                touched.push(id);
            }
            refresh_components |= matches!(current, jian_ops_schema::node::PenNode::Frame(node) if node.reusable == Some(true))
                || matches!(&operation.value, jian_ops_schema::node::PenNode::Frame(node) if node.reusable == Some(true));
            if !scopes.contains(&scope) {
                scopes.push(scope.clone());
            }
            let spec = TargetSpec {
                scope,
                path: operation.path.clone().into_boxed_slice(),
                subtree: false,
                splice: None,
                topology: None,
            };
            if !targets.contains(&spec) {
                targets.push(spec);
            }
        }
        if targets.is_empty() {
            return Ok(None);
        }
        // All preconditions and types are checked before the first assignment;
        // metadata-only operations leave every subsequent structural path valid.
        let pending = PendingScopedSnapshot::capture_targets(self, scopes, false, targets)
            .map_err(|_| Error::Capture)?;
        let history = self.history.clone();
        let history_push_count = self.history_push_count;
        for mut operation in batch.operations {
            let scope = match operation.page {
                None => patch::Scope::Root,
                Some(page) => patch::Scope::Page {
                    index: page.index,
                    id: page.id,
                },
            };
            let current = patch::node_mut(
                patch::roots_mut(&mut self.doc, &scope).unwrap(),
                &operation.path,
            )
            .unwrap();
            let children = patch::children_slot(current).and_then(Option::take);
            if let Some(slot) = patch::children_slot(&mut operation.value) {
                *slot = children;
            }
            *current = operation.value;
        }
        let mut snapshot = pending
            .compact(self)
            .expect("validated metadata feed keeps page/node shape");
        if snapshot.patches.is_empty() {
            return Ok(None);
        }
        // Match the whole-snapshot pull's derived-state refresh, while retaining
        // the live canonical document and its existing child allocations.
        if refresh_components {
            self.components = crate::ComponentLibrary::from_document(&self.doc);
        }
        for id in touched {
            self.ui.variables.fill_refs.remove(&id);
            self.ui.variables.stroke_refs.remove(&id);
        }
        self.editor_ui.clear_document_derived();
        self.codegen.reset_for_document_replacement();
        self.instance_write_virtual_anchor = None;
        jian_ops_schema::image_thumbs::activate_for_document(&self.doc);
        // Derived component/token caches are refreshed after compaction. Keep
        // this external pull conservative for later actor-selective undo.
        snapshot.actor_safe = false;
        let snapshot = Arc::new(snapshot);
        self.history_push_entry(HistoryEntry::Scoped(Arc::clone(&snapshot)));
        Ok(Some(CommittedMetadataApply {
            snapshot,
            history,
            generation: self.document_generation(),
            post_revision: self.document_revision(),
            history_push_count,
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
        self.editor_ui.clear_document_derived();
        self.codegen.reset_for_document_replacement();
        Ok(())
    }
}
