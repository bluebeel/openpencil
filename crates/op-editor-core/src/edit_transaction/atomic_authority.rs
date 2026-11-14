//! Single execution against a complete, exclusively held document authority.
//! Proven edits retain touched roots once; broader writes upgrade their exact
//! original forests. Both seal through existing scoped history, never simulation.
use crate::command_batch::actor::BatchActorSnapshot;
use crate::command_id_scope::AtomicCommandAllocator;
use crate::history::PendingScopedSnapshot;
use crate::{
    EditorCommand, EditorState, History, HistoryEntry, IdAllocError, ScopedEditorSnapshot,
    ScopedHistoryError,
};
use std::{collections::BTreeMap, sync::Arc};
#[path = "atomic_components.rs"]
mod components;
#[path = "atomic_identities.rs"]
mod identities;

#[derive(Debug)]
pub enum AtomicAuthorityError {
    Unsupported,
    Rejected,
    InvalidPhase,
    Panicked,
    Allocation(IdAllocError),
    Scoped(ScopedHistoryError),
    IncompleteRollback,
    IdentityLimit { count: usize, limit: usize },
}
impl std::fmt::Display for AtomicAuthorityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => f.write_str("command requires the complete legacy atomic path"),
            Self::Rejected => f.write_str("subtool command rejected during atomic execution"),
            Self::InvalidPhase => f.write_str("atomic authority edit is already sealed"),
            Self::Panicked => f.write_str("atomic subtool execution panicked"),
            Self::Allocation(error) => write!(f, "{error}"),
            Self::Scoped(error) => write!(f, "{error}"),
            Self::IdentityLimit { count, limit } => write!(
                f,
                "one operation introduces {count} identities; limit is {limit}"
            ),
            Self::IncompleteRollback => f.write_str(
                "atomic rollback is incomplete; reload the document before further writes",
            ),
        }
    }
}
impl std::error::Error for AtomicAuthorityError {}

#[must_use = "accept or abort the atomic authority edit before acknowledging it"]
pub struct AtomicAuthorityEdit {
    pending: Option<PendingScopedSnapshot>,
    completed: Option<Arc<ScopedEditorSnapshot>>,
    allocator: AtomicCommandAllocator,
    history: Option<History>,
    actor: Option<BatchActorSnapshot>,
    active_theme: BTreeMap<String, String>,
    revision: u64,
    runtime_high_water: u64,
    marks_document: bool,
    identities: Option<identities::IdentityTracker>,
}
impl AtomicAuthorityEdit {
    /// The caller must own the complete writer/actor authority until accept
    /// or known rollback. Public mutable documents/custom callbacks must not
    /// use this provenance-sensitive command classification path.
    pub fn begin(state: &EditorState) -> Result<Self, AtomicAuthorityError> {
        if state.undo_capture_suspended
            || state.editor_ui.workspace.active
            || state.runtime_allocator_depth != 0
        {
            return Err(AtomicAuthorityError::Unsupported);
        }
        Ok(Self {
            pending: Some(PendingScopedSnapshot::capture_incremental_preserving_components(state)),
            completed: None,
            allocator: AtomicCommandAllocator::new(state),
            history: Some(state.history.clone()),
            actor: Some(BatchActorSnapshot::capture(state, &[])),
            active_theme: state.ui.variables.active_theme.clone(),
            revision: state.document_revision(),
            runtime_high_water: state.runtime_node_high_water,
            marks_document: false,
            identities: None,
        })
    }
    /// Call before each resolved MCP subtool, not before every command it emits.
    pub fn begin_identity_step(&mut self) {
        if let Some(identities) = &mut self.identities {
            identities.begin_step();
        }
    }
    pub fn finish_identity_step(
        &mut self,
        state: &EditorState,
    ) -> Result<Vec<String>, AtomicAuthorityError> {
        let ids = match &mut self.identities {
            Some(tracker) => tracker
                .finish_step(&state.doc)
                .map_err(AtomicAuthorityError::Scoped)?,
            None => Vec::new(),
        };
        if ids.len() > 16384 {
            return Err(AtomicAuthorityError::IdentityLimit {
                count: ids.len(),
                limit: 16384,
            });
        }
        Ok(ids)
    }
    pub fn apply(
        &mut self,
        state: &mut EditorState,
        command: EditorCommand,
    ) -> Result<(), AtomicAuthorityError> {
        if !command.supports_atomic_batch() {
            return Err(AtomicAuthorityError::Unsupported);
        }
        let (mut scopes, header) =
            super::command_capture::incremental_atomic_scope(state, &command)
                .ok_or(AtomicAuthorityError::Unsupported)?;
        // The existing dirty classification guarantees these commands cannot
        // mutate canonical forests; their actor/selection state is captured.
        if !command.may_change_document() {
            scopes.clear();
        }
        let targets = super::command_capture::incremental_atomic_targets(state, &command);
        if !components::preserves(state, &command, targets.as_deref()) {
            self.pending
                .as_mut()
                .ok_or(AtomicAuthorityError::InvalidPhase)?
                .upgrade_component_checkpoint(state);
        }
        let preserves_cursor = targets.as_ref().is_some_and(|targets| {
            targets
                .iter()
                .all(|spec| !spec.subtree && spec.topology.is_none())
        });
        let observation = if command.may_change_document_identities() {
            Some(
                self.identities
                    .get_or_insert_with(|| identities::IdentityTracker::new(&state.doc))
                    .observe_before(state, &command, targets.as_deref(), &scopes)
                    .map_err(AtomicAuthorityError::Scoped)?,
            )
        } else {
            None
        };
        self.pending
            .as_mut()
            .ok_or(AtomicAuthorityError::InvalidPhase)?
            .extend_before_change(state, scopes, header, targets.as_deref())
            .map_err(AtomicAuthorityError::Scoped)?;
        self.actor
            .as_mut()
            .ok_or(AtomicAuthorityError::InvalidPhase)?
            .capture_clipboard_before(state, &command);
        let marks = crate::command_apply::command_marks_document_dirty(&command);
        let suspended = std::mem::replace(&mut state.undo_capture_suspended, true);
        let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.allocator
                .apply_with_identity_proof(state, command, preserves_cursor)
        }));
        state.undo_capture_suspended = suspended;
        match applied {
            Ok(Ok(true)) => {
                if let Some(observation) = observation {
                    self.identities
                        .as_mut()
                        .unwrap()
                        .observe_after(state, observation)
                        .map_err(AtomicAuthorityError::Scoped)?;
                }
                self.marks_document |= marks;
                Ok(())
            }
            Ok(Ok(false)) => Err(AtomicAuthorityError::Rejected),
            Ok(Err(error)) => Err(AtomicAuthorityError::Allocation(error)),
            Err(_) => Err(AtomicAuthorityError::Panicked),
        }
    }
    /// Install one existing scoped undo entry before durable persistence.
    /// Keep the exact inverse until the host confirms its atomic write.
    pub fn seal(&mut self, state: &mut EditorState) -> Result<(), AtomicAuthorityError> {
        let pending = self
            .pending
            .as_ref()
            .ok_or(AtomicAuthorityError::InvalidPhase)?;
        pending
            .validate_incremental(state)
            .map_err(AtomicAuthorityError::Scoped)?;
        // Validation above proves every source forest is still addressable;
        // no callback can run between it and this consuming compaction.
        let snapshot = self
            .pending
            .take()
            .unwrap()
            .compact(state)
            .map_err(AtomicAuthorityError::Scoped)?;
        let snapshot = Arc::new(snapshot);
        state.history = self
            .history
            .as_ref()
            .ok_or(AtomicAuthorityError::InvalidPhase)?
            .clone();
        if self.marks_document {
            // Match the normal Batch boundary: intermediate revisions remain
            // visible to reads, then one batch mark and one history push seal.
            state.revision = self.revision;
            state.sync_dirty_flag();
            state.mark_document_changed();
            state.history_push_entry(HistoryEntry::Scoped(Arc::clone(&snapshot)));
        }
        self.completed = Some(snapshot);
        Ok(())
    }
    pub fn accept(self) {}
    pub fn abort(mut self, state: &mut EditorState) -> Result<(), AtomicAuthorityError> {
        let restored = if let Some(mut pending) = self.pending.take() {
            pending.restore(state)
        } else if let Some(snapshot) = self.completed.take() {
            // Release the just-created undo entry before exchanging its
            // inverse, so unchanged shared payloads need no redundant clone.
            if let Some(history) = self.history.take() {
                state.history = history;
            }
            Arc::unwrap_or_clone(snapshot).exchange(state)
        } else {
            return Err(AtomicAuthorityError::IncompleteRollback);
        };
        restored.map_err(AtomicAuthorityError::Scoped)?;
        if let Some(history) = self.history.take() {
            state.history = history;
        }
        state.editor_ui.clear_document_derived();
        state.clear_document_drafts(self.active_theme);
        state.instance_write_virtual_anchor = None;
        jian_ops_schema::image_thumbs::activate_for_document(&state.doc);
        if let Some(actor) = self.actor.take() {
            actor.restore(state);
        }
        // IDs minted only by this unacknowledged exclusive plan were never
        // published. Restore its initial floor before an exact legacy retry.
        state.runtime_node_high_water = self.runtime_high_water;
        Ok(())
    }
}
