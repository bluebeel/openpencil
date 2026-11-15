//! Undo / redo stacks for the editor.
//!
//! Models `openpencil-shell-core::document`'s `History` +
//! `DocumentSnapshot` pair, retargeted onto the canonical document
//! model: snapshots recursively share unchanged document nodes and component
//! metadata, alongside the selection and active-page index.
//!
//! In shell-core the snapshot had a separate `var_table` field because
//! variables lived outside the node model; here `PenDocument` carries them
//! with the canonical document metadata.

use crate::history_snapshot::{SharedComponents, SharedDoc};
use crate::node_id::NodeId;
use crate::selection::SelectionState;
use crate::state::EditorState;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
mod actor;
mod entry;
mod retention;
mod scoped;
pub use actor::{ActorHistoryError, HistoryActor, HistoryTag};
pub use entry::HistoryEntry;
pub use retention::{HistoryLimitError, HistoryLimits, HistoryRetentionStatus};
pub(crate) use scoped::{
    auxiliary_command_forecast, auxiliary_command_supported, boolean_command_bound,
    contains_header_token, deletion_command_bound, structural_payload_bound, PendingScopedSnapshot,
    Scope, SpliceSpec, TargetSpec, TopologyKind, TopologyLocation, TopologyRequest, TopologySpec,
};
pub use scoped::{CommittedMetadataApply, ScopedEditorSnapshot, ScopedHistoryError};

/// Document-dependent comparison state. Camera and other view-only state stay
/// outside history; picking a direction must be undoable as a usable comparison.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WorkspaceDirectionsSnapshot {
    run_epoch: u64,
    count: u8,
    variants: Arc<Vec<crate::WorkspaceVariant>>,
}

/// Largest number of undo entries kept. Past this the oldest entry is
/// dropped (`VecDeque::pop_front`) — matches shell-core's cap.
pub const HISTORY_CAP: usize = 100;

/// Snapshot of the editor state covered by undo / redo.
///
/// Holds a **structurally-shared** view of the canonical document
/// ([`SharedDoc`] — unchanged descendants shared by `Arc` across adjacent
/// snapshots) and the component prototypes ([`SharedComponents`]) plus
/// the transient registries whose behavior depends on document edits.
/// A variable-table edit can therefore be undone the same way a node
/// edit can, and component promotion stays in sync with the persisted
/// `reusable` flag. See [`crate::history_snapshot`] for the sharing +
/// copy-on-write rules; snapshots materialize back to an owned
/// `PenDocument` on every restore.
#[derive(Debug, Clone, PartialEq)]
pub struct EditorSnapshot {
    /// Runtime-only operation ownership; never part of the canonical schema.
    pub history_tag: HistoryTag,
    /// The canonical document at snapshot time, shared recursively.
    pub doc: SharedDoc,
    /// Selection at snapshot time.
    pub selection: SelectionState,
    /// Active page index at snapshot time.
    pub active_page_index: usize,
    /// Runtime component registry mirrored from reusable document nodes
    /// and explicit component commands, shared by `Arc` per prototype.
    pub components: SharedComponents,
    /// `MergeAppState` ownership map (`key → owning plan_idx`) at
    /// snapshot time. Must travel with the snapshot: doc.state is
    /// restored on undo / batch rollback, so ownership left behind
    /// would mark keys as generation-owned that the restored document
    /// no longer carries — later merges would be silently skipped or
    /// mis-resolved against a stale owner.
    pub app_state_owner: std::collections::BTreeMap<String, usize>,
    /// Primary fill-variable cache at snapshot time. The cache is
    /// document-derived but must round-trip with undo when an edit changes
    /// which authored fill occupies index 0.
    pub fill_refs: HashMap<NodeId, String>,
    /// Stroke-variable companion to [`Self::fill_refs`]. Keeping both caches
    /// together prevents a history restore from producing asymmetric token
    /// resolution state.
    pub stroke_refs: HashMap<NodeId, String>,
    /// Whether the current document must keep its authored coordinates instead
    /// of running flex layout. Layout mutations can clear this document-level
    /// latch, so undo / redo must restore it alongside the document snapshot.
    pub preserve_authored_geometry: bool,
    pub(crate) workspace_directions: Option<WorkspaceDirectionsSnapshot>,
    /// Document revision at snapshot time. Restoring it lets undo back
    /// to a saved snapshot clear the dirty marker naturally.
    pub revision: u64,
}

/// Editor undo / redo stacks. `VecDeque` so the over-cap eviction is an
/// O(1) `pop_front` rather than an O(n) `Vec::remove(0)`.
#[derive(Debug, Clone, Default)]
pub struct History {
    pub past: VecDeque<HistoryEntry>,
    pub future: VecDeque<HistoryEntry>,
    actor: Option<HistoryActor>,
    actor_error: Option<ActorHistoryError>,
    conflicts: std::collections::BTreeSet<u64>,
    limits: HistoryLimits,
    mixed_actors: bool,
    retention: retention::HistoryRetention,
}

impl History {
    /// An empty history — no undo, no redo.
    pub fn new() -> Self {
        Self::default()
    }

    /// True when there is at least one undo entry.
    pub fn can_undo(&self) -> bool {
        self.validate_actor(false).is_ok()
    }

    /// True when there is at least one redo entry.
    pub fn can_redo(&self) -> bool {
        self.validate_actor(true).is_ok()
    }
}

impl EditorState {
    /// Group a synchronous document operation into one undo step. Individual
    /// validated writes retain their normal behavior; only intermediate history
    /// entries are collapsed and revision-only writes gain a recovery snapshot.
    /// The closure must not call undo/redo or reset the
    /// document. A no-op preserves both history stacks, including existing redo.
    /// If the closure unwinds, history bookkeeping is restored and any changed
    /// prefix remains undoable as one step before the panic resumes.
    pub fn with_history_group<T>(&mut self, operation: impl FnOnce(&mut Self) -> T) -> T {
        self.with_history_group_anchored(None, operation)
    }

    pub(crate) fn with_history_group_anchored<T>(
        &mut self,
        anchor: Option<&EditorSnapshot>,
        operation: impl FnOnce(&mut Self) -> T,
    ) -> T {
        self.with_history_checkpoint(anchor, |state, _| operation(state))
    }

    /// The callback receives a checkpoint captured immediately before it runs.
    /// Only a direct command may forward it to its own rollback path; a nested
    /// group receives None because an earlier callback may already have edited.
    pub(crate) fn with_history_checkpoint<T>(
        &mut self,
        anchor: Option<&EditorSnapshot>,
        operation: impl FnOnce(&mut Self, Option<&EditorSnapshot>) -> T,
    ) -> T {
        // A containing operation already owns the checkpoint. This also keeps
        // nested helper groups from repeatedly capturing the same document.
        if self.undo_capture_suspended {
            return operation(self, None);
        }
        let before = match anchor {
            Some(anchor) => self.snapshot_for_history_with_anchor(Some(anchor)),
            None => self.snapshot_for_history(),
        };
        let history = std::mem::take(&mut self.history);
        // Rollback captures inside a command still need its actual current
        // state. Give them a sharing anchor without exposing the real stacks.
        self.history.past.push_back(before.clone().into());
        self.undo_capture_suspended = true;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            operation(self, Some(&before))
        }));
        self.undo_capture_suspended = false;
        self.history = history;
        if self.revision != before.revision {
            self.history_push_past(before);
        }
        match result {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// Snapshot the editor's undoable state without pushing it.
    ///
    /// The snapshot covers the document, selection, active page, component
    /// registry, and transient variable-reference caches. View-only UI state
    /// such as collapsed layers is intentionally excluded.
    pub fn snapshot_for_history(&self) -> EditorSnapshot {
        // After an undo, the redo back is the state we came from and therefore
        // the closest sharing anchor. Normal forward editing falls through to
        // the previous undo entry.
        let anchor = self
            .history
            .future
            .iter()
            .rev()
            .find_map(HistoryEntry::full_snapshot)
            .or_else(|| {
                self.history
                    .past
                    .iter()
                    .rev()
                    .find_map(HistoryEntry::full_snapshot)
            });
        self.snapshot_for_history_with_anchor(anchor)
    }

    /// [`snapshot_for_history`](Self::snapshot_for_history) with an explicit
    /// adjacent snapshot whose unchanged descendants may be shared.
    pub fn snapshot_for_history_with_anchor(
        &self,
        anchor: Option<&EditorSnapshot>,
    ) -> EditorSnapshot {
        EditorSnapshot {
            history_tag: HistoryTag::capture(self.history.actor),
            doc: SharedDoc::capture(&self.doc, anchor.map(|snapshot| &snapshot.doc)),
            selection: self.selection.clone(),
            active_page_index: self.ui.active_page_index,
            components: SharedComponents::capture(
                &self.components,
                anchor.map(|snapshot| &snapshot.components),
            ),
            app_state_owner: self.app_state_owner.clone(),
            fill_refs: self.ui.variables.fill_refs.clone(),
            stroke_refs: self.ui.variables.stroke_refs.clone(),
            preserve_authored_geometry: self.editor_ui.preserve_authored_geometry,
            workspace_directions: self.capture_workspace_directions(anchor),
            revision: self.revision,
        }
    }

    /// Push a snapshot onto the undo stack, clear redo, and enforce the cap.
    pub fn history_push_past(&mut self, snapshot: EditorSnapshot) {
        if self.undo_capture_suspended {
            self.mark_document_changed();
            return;
        }
        self.history_push_entry(snapshot.into());
    }

    pub(crate) fn history_push_entry(&mut self, entry: HistoryEntry) {
        self.history_push_count = self.history_push_count.saturating_add(1);
        self.history.mixed_actors |= entry.history_tag().actor.is_some();
        self.history.invalidate_redo(&entry, true);
        self.history.past.push_back(entry);
        self.history.enforce_retention();
        self.mark_document_changed();
    }

    /// Push the current state before a transactional edit.
    pub fn commit_history(&mut self) {
        let snapshot = self.snapshot_for_undo();
        self.push_undo(snapshot);
    }

    /// Capture only for undo storage. Callers that compare or restore a local
    /// checkpoint must continue using `snapshot_for_history` instead.
    pub(crate) fn snapshot_for_undo(&self) -> Option<EditorSnapshot> {
        (!self.undo_capture_suspended).then(|| self.snapshot_for_history())
    }

    pub(crate) fn push_undo(&mut self, snapshot: Option<EditorSnapshot>) {
        if let Some(snapshot) = snapshot {
            self.history_push_past(snapshot);
        } else {
            debug_assert!(self.undo_capture_suspended);
            self.mark_document_changed();
        }
    }

    /// Materialize and restore all undoable editor state from a snapshot.
    pub(crate) fn restore(&mut self, snapshot: EditorSnapshot) {
        self.doc = snapshot.doc.materialize();
        self.selection = snapshot.selection;
        self.ui.active_page_index = snapshot.active_page_index;
        self.components = snapshot.components.materialize();
        self.app_state_owner = snapshot.app_state_owner;
        self.ui.variables.fill_refs = snapshot.fill_refs;
        self.ui.variables.stroke_refs = snapshot.stroke_refs;
        self.editor_ui.preserve_authored_geometry = snapshot.preserve_authored_geometry;
        self.restore_workspace_directions(snapshot.workspace_directions);
        self.revision = snapshot.revision;
        self.sync_dirty_flag();
    }

    fn capture_workspace_directions(
        &self,
        anchor: Option<&EditorSnapshot>,
    ) -> Option<WorkspaceDirectionsSnapshot> {
        let workspace = &self.editor_ui.workspace;
        if !workspace.active {
            return None;
        }
        let variants = anchor
            .and_then(|snapshot| snapshot.workspace_directions.as_ref())
            .filter(|snapshot| snapshot.variants.as_ref() == &workspace.variants)
            .map(|snapshot| Arc::clone(&snapshot.variants))
            .unwrap_or_else(|| Arc::new(workspace.variants.clone()));
        Some(WorkspaceDirectionsSnapshot {
            run_epoch: workspace.run_epoch,
            count: workspace.variant_count,
            variants,
        })
    }

    fn restore_workspace_directions(&mut self, snapshot: Option<WorkspaceDirectionsSnapshot>) {
        let Some(snapshot) = snapshot else { return };
        let workspace = &mut self.editor_ui.workspace;
        // History from another run must never bring its old chooser into a new
        // generation. Professional/reading mode transitions remain view state.
        if !workspace.active || workspace.run_epoch != snapshot.run_epoch {
            return;
        }
        if workspace.variant_count == snapshot.count
            && &workspace.variants == snapshot.variants.as_ref()
        {
            return;
        }
        workspace.variant_count = snapshot.count;
        workspace.variants = snapshot.variants.as_ref().clone();
        workspace.view = if workspace.variants.is_empty() {
            crate::WorkspaceView::default_for(workspace.family)
        } else {
            crate::WorkspaceView::AllBoards
        };
        workspace.selected = 0;
        workspace.fitted_board_count = 0;
        workspace.fitted_bounds = None;
    }

    /// Undo this actor's latest disjoint edit, or the latest global edit when
    /// no host actor context is installed. Conflicts never mutate the document.
    pub fn undo(&mut self) -> bool {
        self.exchange_history(false)
    }
    /// Redo this actor's retained branch, preserving unrelated actors' branches.
    pub fn redo(&mut self) -> bool {
        self.exchange_history(true)
    }
}

fn rebase_scoped_prefix(entries: &mut VecDeque<HistoryEntry>, generation: u64) {
    for entry in entries.iter_mut().rev() {
        let HistoryEntry::Scoped(snapshot) = entry else {
            break;
        };
        ScopedEditorSnapshot::rebase_generation(snapshot, generation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_history_has_no_undo_or_redo() {
        let h = History::new();
        assert!(!h.can_undo());
        assert!(!h.can_redo());
    }
}

impl EditorSnapshot {
    /// Heap copied by History::clone; document/component/workspace owners are
    /// immutable Arc graphs and are accounted separately by the group ledger.
    pub fn command_clone_bytes(&self) -> usize {
        let ids = |selection: &SelectionState| {
            selection.set.len() * std::mem::size_of::<NodeId>()
                + selection
                    .set
                    .iter()
                    .map(|id| 64 + 8 * id.as_str().len())
                    .sum::<usize>()
                + 64
                + 8 * selection.anchor.as_str().len()
        };
        let refs = |map: &HashMap<NodeId, String>| {
            map.capacity() * (std::mem::size_of::<(NodeId, String)>() + 32) * 2
                + map
                    .iter()
                    .map(|(key, value)| 64 + 8 * (key.as_str().len() + value.len()))
                    .sum::<usize>()
        };
        std::mem::size_of::<Self>()
            + ids(&self.selection)
            + refs(&self.fill_refs)
            + refs(&self.stroke_refs)
            + self.app_state_owner.len() * 128
            + self
                .app_state_owner
                .keys()
                .map(|key| 64 + key.len() * 8)
                .sum::<usize>()
    }
    pub fn command_workspace_identity(&self) -> Option<usize> {
        self.workspace_directions
            .as_ref()
            .map(|workspace| Arc::as_ptr(&workspace.variants) as usize)
    }
    pub fn command_workspace_retention(
        &self,
        limit: usize,
    ) -> Result<Option<(usize, usize)>, crate::history_snapshot::CaptureSizeError> {
        let Some(workspace) = &self.workspace_directions else {
            return Ok(None);
        };
        let mut bytes = std::mem::size_of_val(workspace.variants.as_ref());
        for variant in workspace.variants.iter() {
            let metadata = (
                &variant.name,
                &variant.style_guide,
                &variant.style_label,
                &variant.name_prefix,
                &variant.root_ids,
                &variant.variables,
                &variant.themes,
            );
            bytes = bytes.saturating_add(crate::history_snapshot::command_capture_value_bytes(
                &metadata,
                limit.saturating_sub(bytes),
            )?);
        }
        Ok(Some((Arc::as_ptr(&workspace.variants) as usize, bytes)))
    }
}
