//! Trusted command transactions with compact page-local rollback/history.
use super::*;
use crate::{
    history::PendingScopedSnapshot, EditorCommand, HistoryEntry, ScopedEditorSnapshot,
    ScopedHistoryError,
};
use std::{mem, sync::Arc};
#[path = "command_auxiliary_apply.rs"]
mod auxiliary;
#[path = "command_scope.rs"]
mod command_scope;
#[path = "command_history_cohort.rs"]
mod history_cohort;
#[path = "command_metadata_apply.rs"]
mod metadata;
#[path = "target_plan.rs"]
mod target_plan;

/// Authority-only classification; no command executes and no full fallback
/// capture is allocated. Unknown/unstable scopes remain on legacy dispatch.
pub(crate) fn incremental_atomic_scope(
    state: &EditorState,
    command: &EditorCommand,
) -> Option<(Vec<crate::history::Scope>, bool)> {
    match command_scope::classify(state, command) {
        command_scope::Classification::Scoped(plan) => Some((plan.forests, plan.header)),
        _ => None,
    }
}

#[derive(Debug)]
pub struct CommandEditCapture {
    kind: CaptureKind,
    batch_actor: Option<crate::command_batch::actor::BatchActorSnapshot>,
    // Abort-only actor state: ordinary undo/redo deliberately leaves clipboard.
    rollback_clipboard: Option<ClipboardRollback>,
}
#[derive(Debug)]
enum ClipboardRollback {
    Copy(Vec<jian_ops_schema::node::PenNode>),
    Moved(Vec<jian_ops_schema::node::PenNode>),
}
#[derive(Debug)]
enum CaptureKind {
    Full(LocalEditCapture),
    Refused { generation: u64 },
    Scoped(ScopedCapture),
    History(HistoryCapture),
}
#[derive(Debug)]
struct ScopedCapture {
    targeted: bool,
    generation: u64,
    revision: u64,
    pending: Option<PendingScopedSnapshot>,
    completed: Option<Arc<ScopedEditorSnapshot>>,
    history: History,
    active_theme: BTreeMap<String, String>,
}
#[derive(Debug)]
struct HistoryCapture {
    actor_selective: bool,
    started: bool,
    generation: u64,
    revision: u64,
    undo: bool,
    history: History,
    completed: Option<Arc<ScopedEditorSnapshot>>,
    active_theme: BTreeMap<String, String>,
}
#[derive(Debug)]
pub enum CommandEditError {
    ClipboardContextChanged,
    Full(LocalEditError),
    Scoped(ScopedHistoryError),
    IncompleteScopedCapture,
}
impl std::fmt::Display for CommandEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClipboardContextChanged => f.write_str(
                "the cut clipboard checkpoint belongs to a different document or actor context",
            ),
            Self::Full(error) => write!(f, "{error}"),
            Self::Scoped(error) => write!(f, "{error}"),
            Self::IncompleteScopedCapture => f.write_str(
                "trusted command rollback capture did not complete; document requires recovery",
            ),
        }
    }
}
impl std::error::Error for CommandEditError {}
impl CommandEditCapture {
    /// Visit exact old/new headers immediately after a trusted target edit.
    /// True proves only metadata Node patches: child buffers are unchanged.
    /// Full, structural, broad-maintenance and history transactions return false.
    /// This is not a proof for arbitrary intervening public document mutation.
    pub fn visit_metadata_changes(
        &self,
        state: &EditorState,
        mut visitor: impl FnMut(&jian_ops_schema::node::PenNode, &jian_ops_schema::node::PenNode),
    ) -> bool {
        match &self.kind {
            CaptureKind::Scoped(captured) if captured.targeted => captured
                .completed
                .as_ref()
                .is_some_and(|snapshot| snapshot.visit_metadata_changes(state, &mut visitor)),
            _ => false,
        }
    }

    /// Exact bounded forward canonical metadata from a completed trusted edit.
    /// Call immediately after apply/maintenance, before another command changes
    /// this document. None requires a resync barrier, never command replay.
    pub fn forward_metadata_json(&self, state: &EditorState, limit: usize) -> Option<Vec<u8>> {
        match &self.kind {
            CaptureKind::Scoped(captured) => captured
                .completed
                .as_ref()?
                .forward_metadata_json(state, limit),
            CaptureKind::History(captured) => captured
                .completed
                .as_ref()?
                .forward_metadata_json(state, limit),
            CaptureKind::Full(_) | CaptureKind::Refused { .. } => None,
        }
    }

    fn clipboard_context_matches(&self, state: &EditorState) -> bool {
        self.rollback_clipboard.is_none()
            || match &self.kind {
                CaptureKind::Scoped(captured) => {
                    captured.generation == state.document_generation()
                        && captured.history.actor() == state.history.actor()
                }
                _ => false,
            }
    }
    /// This command owns its actor's prior clipboard until commit/abort.
    pub fn owns_replaced_clipboard_rollback(&self) -> bool {
        self.rollback_clipboard.is_some()
    }
    pub fn is_scoped(&self) -> bool {
        matches!(self.kind, CaptureKind::Scoped(_) | CaptureKind::History(_))
    }
    pub fn is_target_only(&self) -> bool {
        match &self.kind {
            CaptureKind::Scoped(captured) => captured.targeted,
            CaptureKind::History(_) | CaptureKind::Refused { .. } => true,
            CaptureKind::Full(_) => false,
        }
    }
}
impl EditorState {
    /// Forecast the bounded affected structural payload before any owned
    /// rollback/history clone. None keeps the caller's ordinary isolated path.
    pub fn forecast_structural_command_edit(&self, command: &EditorCommand) -> Option<usize> {
        target_plan::structural_bound(self, command)
    }
    /// Exact current no-catalog-write proof shared with the live atomic driver.
    /// No revision cache survives this borrow or a later canonical mutation.
    pub fn command_edit_preserves_components(&self, command: &EditorCommand) -> bool {
        let targets = target_plan::plan(self, command);
        super::component_proof::preserves(self, command, targets.as_deref())
    }

    /// New-content-only generation repair admission. Existing-target refine or
    /// structural commands outside the static append proof retain broad capture.
    pub fn command_repair_capture_bound(&self, command: &EditorCommand) -> Option<usize> {
        let command_scope::Classification::Scoped(plan) = command_scope::classify(self, command)
        else {
            return None;
        };
        if plan.header {
            return None;
        }
        let specs = target_plan::plan(self, command)?;
        if !specs.iter().any(|spec| spec.splice.is_some())
            || specs
                .iter()
                .any(|spec| spec.subtree || spec.topology.is_some())
        {
            return None;
        }
        let mut commands = vec![command];
        let mut nodes = Vec::new();
        let mut count = 0usize;
        while let Some(command) = commands.pop() {
            match command {
                EditorCommand::Batch { commands: members } => commands.extend(members),
                EditorCommand::InsertSubtree {
                    nodes: incoming, ..
                }
                | EditorCommand::InsertAuthoredSubtree {
                    nodes: incoming, ..
                }
                | EditorCommand::InsertAuthoredSubtreePreservingRoots {
                    nodes: incoming, ..
                } => nodes.extend(incoming),
                EditorCommand::BatchInsert { items, .. } => {
                    count = count.checked_add(items.len())?
                }
                EditorCommand::InsertNode { .. } => count = count.checked_add(1)?,
                _ => {}
            }
        }
        while let Some(node) = nodes.pop() {
            count = count.checked_add(1)?;
            if count > 16_384 {
                return None;
            }
            if let Some(children) = crate::PenNodeExt::children(node) {
                nodes.extend(children);
            }
        }
        // Repairs move existing/new payload rather than clone a document. This
        // covers bounded default status-bar/image-slot expansion per input node;
        // complete input/path resources and sibling tables are charged separately.
        count
            .checked_mul(128 * 1024)
            .filter(|bytes| *bytes <= crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE)
    }

    /// Capture before an exclusively owned host command. The caller guarantees
    /// synchronous maintenance changes only the original active page; known
    /// commands also capture their explicitly requested page. Audited metadata commands capture exact
    /// header fields too; unclassified global
    /// commands keep the complete existing LocalEditCapture path. Public raw
    /// document writers and arbitrary callbacks must keep begin_local_edit.
    pub fn begin_command_edit(&self, command: &EditorCommand) -> CommandEditCapture {
        let kind = match command_scope::classify(self, command) {
            command_scope::Classification::Scoped(plan) => CaptureKind::Scoped(ScopedCapture {
                targeted: false,
                generation: self.document_generation,
                revision: self.revision,
                pending: Some(PendingScopedSnapshot::capture(
                    self,
                    plan.forests,
                    plan.header,
                )),
                completed: None,
                history: self.history.clone(),
                active_theme: self.ui.variables.active_theme.clone(),
            }),
            command_scope::Classification::Full => CaptureKind::Full(self.begin_local_edit()),
            command_scope::Classification::Refused => CaptureKind::Refused {
                generation: self.document_generation,
            },
        };
        CommandEditCapture {
            kind,
            rollback_clipboard: None,
            batch_actor: crate::command_batch::actor::BatchActorSnapshot::for_command(
                self, command,
            ),
        }
    }
    /// Trusted host variant: the synchronous maintenance callback must not
    /// mutate canonical document content. Existing begin_command_edit keeps
    /// its broader page-local callback contract. Unsupported/dynamic commands
    /// deliberately retain that complete fallback.
    pub fn begin_command_edit_without_document_repairs(
        &self,
        command: &EditorCommand,
    ) -> CommandEditCapture {
        self.try_begin_command_edit_without_document_repairs(command)
            .unwrap_or_else(|| self.begin_command_edit(command))
    }
    /// Query this bounded capture path without allocating a legacy page/full
    /// fallback. None means the caller must use its separate ordinary path.
    pub fn try_begin_command_edit_without_document_repairs(
        &self,
        command: &EditorCommand,
    ) -> Option<CommandEditCapture> {
        self.try_begin_command_edit_inner(command, true, false)
    }
    /// Synchronous hosts may move the actor clipboard into the checkpoint
    /// instead of deep-cloning it. Apply immediately or explicitly abort this
    /// capture under the same actor context; an unapplied capture must not drop.
    pub fn try_begin_command_edit_transferring_clipboard(
        &mut self,
        command: &EditorCommand,
    ) -> Option<CommandEditCapture> {
        self.try_begin_transferring_clipboard_inner(command, false)
    }
    /// Known synchronous hosts may preserve catalog state for audited child
    /// metadata. Definition roots, owned prototypes and structural commands
    /// retain complete catalog checkpoints. Maintenance must not mutate the
    /// catalog; arbitrary/public callback paths keep the default constructor.
    /// Cut also transfers its old clipboard: apply immediately or abort under
    /// the same actor context, as with the explicit transfer factory.
    pub fn try_begin_command_edit_preserving_components(
        &mut self,
        command: &EditorCommand,
    ) -> Option<CommandEditCapture> {
        self.try_begin_transferring_clipboard_inner(command, true)
    }
    fn try_begin_transferring_clipboard_inner(
        &mut self,
        command: &EditorCommand,
        preserve_components: bool,
    ) -> Option<CommandEditCapture> {
        let mut capture = self.try_begin_command_edit_inner(command, false, preserve_components)?;
        if matches!(command, EditorCommand::CutSelected)
            && matches!(&capture.kind, CaptureKind::Scoped(_))
        {
            capture.rollback_clipboard =
                Some(ClipboardRollback::Moved(mem::take(&mut self.clipboard)));
        }
        Some(capture)
    }
    fn try_begin_command_edit_inner(
        &self,
        command: &EditorCommand,
        clone_clipboard: bool,
        preserve_components: bool,
    ) -> Option<CommandEditCapture> {
        if preserve_components && crate::history::auxiliary_command_supported(command) {
            return self.prepare_auxiliary_command_edit(command);
        }
        let plan = match command_scope::classify(self, command) {
            command_scope::Classification::Scoped(plan) => plan,
            command_scope::Classification::Refused => {
                return Some(CommandEditCapture {
                    rollback_clipboard: None,
                    batch_actor: None,
                    kind: CaptureKind::Refused {
                        generation: self.document_generation,
                    },
                })
            }
            command_scope::Classification::Full => return None,
        };
        if matches!(
            command,
            EditorCommand::DeleteSelected | EditorCommand::CutSelected
        ) {
            let captures_components =
                !preserve_components || !super::component_proof::preserves(self, command, None);
            let pending = PendingScopedSnapshot::capture_deletion(
                self,
                plan.forests,
                command,
                captures_components,
            )?;
            return Some(CommandEditCapture {
                rollback_clipboard: (clone_clipboard
                    && matches!(command, EditorCommand::CutSelected))
                .then(|| ClipboardRollback::Copy(self.clipboard.clone())),
                batch_actor: None,
                kind: CaptureKind::Scoped(ScopedCapture {
                    targeted: true,
                    generation: self.document_generation,
                    revision: self.revision,
                    pending: Some(pending),
                    completed: None,
                    history: self.history.clone(),
                    active_theme: self.ui.variables.active_theme.clone(),
                }),
            });
        }
        if matches!(command, EditorCommand::ReplacePathsWithSvg(_)) {
            let pending = PendingScopedSnapshot::capture_boolean(self, plan.forests, command)?;
            return Some(CommandEditCapture {
                rollback_clipboard: None,
                batch_actor: None,
                kind: CaptureKind::Scoped(ScopedCapture {
                    targeted: true,
                    generation: self.document_generation,
                    revision: self.revision,
                    pending: Some(pending),
                    completed: None,
                    history: self.history.clone(),
                    active_theme: self.ui.variables.active_theme.clone(),
                }),
            });
        }
        if matches!(
            command,
            EditorCommand::DuplicateSelected { .. } | EditorCommand::PasteClipboard { .. }
        ) {
            if self.instance_write_virtual_anchor.is_some() {
                return None;
            }
            let captures_components =
                !preserve_components || !super::component_proof::preserves(self, command, None);
            let pending = PendingScopedSnapshot::capture_clones(
                self,
                plan.forests,
                command,
                captures_components,
            )?;
            return Some(CommandEditCapture {
                rollback_clipboard: None,
                batch_actor: None,
                kind: CaptureKind::Scoped(ScopedCapture {
                    targeted: true,
                    generation: self.document_generation,
                    revision: self.revision,
                    pending: Some(pending),
                    completed: None,
                    history: self.history.clone(),
                    active_theme: self.ui.variables.active_theme.clone(),
                }),
            });
        }
        if matches!(command, EditorCommand::Undo | EditorCommand::Redo) {
            return Some(CommandEditCapture {
                rollback_clipboard: None,
                batch_actor: None,
                kind: CaptureKind::History(HistoryCapture {
                    started: false,
                    actor_selective: self
                        .history
                        .next_entry(matches!(command, EditorCommand::Redo))
                        .is_some_and(|entry| self.history.selective_exchange(entry)),
                    generation: self.document_generation,
                    revision: self.revision,
                    undo: matches!(command, EditorCommand::Undo),
                    history: self.history.clone(),
                    completed: None,
                    active_theme: self.ui.variables.active_theme.clone(),
                }),
            });
        }
        let Some(specs) = target_plan::plan(self, command) else {
            return None;
        };
        let keep_catalog = preserve_components
            && !plan.header
            && super::component_proof::preserves(self, command, Some(&specs));
        let pending = if keep_catalog {
            PendingScopedSnapshot::capture_targets_preserving_components(self, plan.forests, specs)
        } else {
            PendingScopedSnapshot::capture_targets(self, plan.forests, plan.header, specs)
        }
        .ok()?;
        Some(CommandEditCapture {
            rollback_clipboard: None,
            batch_actor: crate::command_batch::actor::BatchActorSnapshot::for_command(
                self, command,
            ),
            kind: CaptureKind::Scoped(ScopedCapture {
                targeted: true,
                generation: self.document_generation,
                revision: self.revision,
                pending: Some(pending),
                completed: None,
                history: self.history.clone(),
                active_theme: self.ui.variables.active_theme.clone(),
            }),
        })
    }
    /// Apply against the matching fresh capture. This is the opt-in trusted
    /// counterpart of apply_undoable_captured_then; legacy constructors and
    /// arbitrary closure APIs retain complete snapshot semantics.
    pub fn apply_command_edit_then(
        &mut self,
        command: EditorCommand,
        capture: &mut CommandEditCapture,
        maintain: impl FnOnce(&mut Self),
    ) -> bool {
        self.apply_command_edit_checked(command, capture, move |state| {
            state.invalidate_command_id_sequence();
            maintain(state);
        })
    }
    /// Exact typed command with no canonical maintenance callback. Lexical
    /// cohort ID receipts can survive this audited boundary; ordinary callback
    /// callers continue through apply_command_edit_then and invalidate them.
    pub fn apply_command_edit_without_document_repairs(
        &mut self,
        command: EditorCommand,
        capture: &mut CommandEditCapture,
    ) -> bool {
        self.apply_command_edit_checked(command, capture, |_| {})
    }
    fn apply_command_edit_checked(
        &mut self,
        command: EditorCommand,
        capture: &mut CommandEditCapture,
        maintain: impl FnOnce(&mut Self),
    ) -> bool {
        self.observe_command_id_sequence(&command);
        if !capture.clipboard_context_matches(self) {
            return false;
        }
        let restore_rejected_transfer = matches!(
            &capture.rollback_clipboard,
            Some(ClipboardRollback::Moved(_))
        ) && matches!(&capture.kind,CaptureKind::Scoped(captured) if captured.revision==self.revision);
        let applied = self.apply_command_edit_inner(command, capture, maintain, None);
        if !applied && restore_rejected_transfer {
            if let Some(ClipboardRollback::Moved(clipboard)) = capture.rollback_clipboard.take() {
                self.clipboard = clipboard;
            }
        }
        applied
    }
    fn apply_command_edit_inner(
        &mut self,
        command: EditorCommand,
        capture: &mut CommandEditCapture,
        maintain: impl FnOnce(&mut Self),
        fresh: Option<crate::command_node_attrs::bulk::Prepared>,
    ) -> bool {
        match &mut capture.kind {
            CaptureKind::Refused { .. } => false,
            CaptureKind::Full(full) => self.apply_undoable_captured_then(command, full, maintain),
            CaptureKind::History(captured) => {
                self.apply_history_capture(command, captured, maintain)
            }
            CaptureKind::Scoped(captured) => {
                if captured.generation != self.document_generation
                    || captured.revision != self.revision
                    || captured.pending.is_none()
                {
                    return false;
                }
                if fresh.is_none() {
                    let pending = captured.pending.as_ref().unwrap();
                    if let Some(valid) = pending.auxiliary_request_matches(self, &command) {
                        if !valid
                            || (pending.captures_variable_key()
                                && captured.active_theme != self.ui.variables.active_theme)
                        {
                            return false;
                        }
                    } else {
                        let command_scope::Classification::Scoped(plan) =
                            command_scope::classify(self, &command)
                        else {
                            return false;
                        };
                        if plan.forests != captured.pending.as_ref().unwrap().scopes()
                            || plan.header != captured.pending.as_ref().unwrap().captures_header()
                            || !captured.pending.as_ref().unwrap().matches_header(&self.doc)
                        {
                            return false;
                        }
                        let pending = captured.pending.as_ref().unwrap();
                        if pending.is_targeted() {
                            let request_matches = pending
                                .matches_topology_request(self, &command)
                                .unwrap_or_else(|| {
                                    target_plan::plan(self, &command)
                                        .is_some_and(|specs| pending.matches_target_specs(&specs))
                                });
                            if !request_matches || !pending.matches_targets(self) {
                                return false;
                            }
                        }
                    }
                }
                let mut pending = captured.pending.take().unwrap();
                let undo_redo = matches!(command, EditorCommand::Undo | EditorCommand::Redo);
                let history = (!undo_redo).then(|| mem::take(&mut self.history));
                if !undo_redo {
                    self.undo_capture_suspended = true;
                }
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let applied = if let Some(prepared) = fresh.as_ref() {
                        pending
                            .apply_metadata_direct(self, &command, prepared)
                            .expect("single freshly prepared metadata target")
                    } else if let Some(applied) = pending.apply_topology(self) {
                        applied
                    } else if let EditorCommand::Batch { commands } = command {
                        let batch = EditorCommand::Batch { commands };
                        let mut allocator =
                            if crate::command_apply_legacy::allocation_policy::requires_id_seed(
                                &batch,
                            ) {
                                let Ok(allocator) = self.standalone_id_allocator() else {
                                    return false;
                                };
                                allocator
                            } else {
                                crate::DocumentIdAllocator::sequential(1)
                            };
                        let EditorCommand::Batch { commands } = batch else {
                            unreachable!()
                        };
                        self.observe_runtime_allocations(
                            &mut allocator,
                            |state, allocator| {
                                state.cmd_batch_with_scoped_checkpoint(
                                    commands,
                                    allocator,
                                    &mut pending,
                                )
                            },
                            |changed| *changed,
                        )
                        .unwrap_or(false)
                    } else {
                        self.apply(command)
                    };
                    if applied {
                        maintain(self);
                    }
                    applied
                }));
                if let Some(history) = history {
                    self.undo_capture_suspended = false;
                    self.history = history;
                }
                let snapshot = Arc::new(
                    pending
                        .compact(self)
                        .expect("trusted page-local command changed document/page identity"),
                );
                if !undo_redo && self.revision != captured.revision {
                    self.history_push_entry(HistoryEntry::Scoped(snapshot.clone()));
                }
                captured.completed = Some(snapshot);
                match result {
                    Ok(applied) => applied,
                    Err(panic) => std::panic::resume_unwind(panic),
                }
            }
        }
    }
    fn apply_history_capture(
        &mut self,
        command: EditorCommand,
        captured: &mut HistoryCapture,
        maintain: impl FnOnce(&mut Self),
    ) -> bool {
        if captured.generation != self.document_generation
            || captured.revision != self.revision
            || captured.completed.is_some()
            || (self.history.past != captured.history.past
                || self.history.future != captured.history.future)
            || !matches!(
                (&command, captured.undo),
                (EditorCommand::Undo, true) | (EditorCommand::Redo, false)
            )
        {
            return false;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            captured.started = true;
            let (applied, reverse) = self.exchange_history_retained(!captured.undo);
            captured.completed = reverse;
            captured.started = applied && captured.completed.is_none();
            if applied {
                maintain(self);
            }
            applied
        }));
        match result {
            Ok(applied) => applied,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
    /// Restore document, history and derived UI after external persistence
    /// rejects a trusted command. Atomic batches also restore the actor state
    /// outside document history. Standalone Cut keeps its moved host rollback.
    pub fn abort_command_edit(
        &mut self,
        capture: CommandEditCapture,
    ) -> Result<(), CommandEditError> {
        self.invalidate_command_id_sequence();
        if !capture.clipboard_context_matches(self) {
            return Err(CommandEditError::ClipboardContextChanged);
        }
        let CommandEditCapture {
            kind,
            batch_actor,
            rollback_clipboard,
        } = capture;
        // The actor/generation guard above makes this independent auxiliary
        // restore safe even if the canonical inverse subsequently needs recovery.
        if let Some(ClipboardRollback::Copy(clipboard) | ClipboardRollback::Moved(clipboard)) =
            rollback_clipboard
        {
            self.clipboard = clipboard;
        }
        let result = match kind {
            CaptureKind::Refused { generation } => self
                .ensure_local_edit_generation(generation)
                .map_err(CommandEditError::Full),
            CaptureKind::Full(full) => self.abort_local_edit(full).map_err(CommandEditError::Full),
            CaptureKind::History(captured) => {
                self.ensure_local_edit_generation(captured.generation)
                    .map_err(CommandEditError::Full)?;
                if captured.started {
                    return Err(CommandEditError::IncompleteScopedCapture);
                }
                self.history = captured.history;
                if let Some(snapshot) = captured.completed {
                    self.editor_ui.clear_document_derived();
                    let mut snapshot = Arc::unwrap_or_clone(snapshot);
                    if captured.actor_selective {
                        snapshot
                            .exchange_actor_rollback(self)
                            .map_err(CommandEditError::Scoped)?;
                        self.revision = captured.revision;
                        self.sync_dirty_flag();
                    } else {
                        snapshot.exchange(self).map_err(CommandEditError::Scoped)?;
                    }
                    self.clear_document_drafts(captured.active_theme);
                    self.codegen.reset_for_document_replacement();
                    self.instance_write_virtual_anchor = None;
                    jian_ops_schema::image_thumbs::activate_for_document(&self.doc);
                }
                Ok(())
            }
            CaptureKind::Scoped(captured) => {
                self.ensure_local_edit_generation(captured.generation)
                    .map_err(CommandEditError::Full)?;
                // Apply consumed the pending checkpoint but failed before its
                // exact inverse was completed. Never report a successful abort
                // for canonical state whose rollback outcome is unknown.
                if captured.pending.is_none() && captured.completed.is_none() {
                    return Err(CommandEditError::IncompleteScopedCapture);
                }
                self.history = captured.history;
                if let Some(snapshot) = captured.completed {
                    self.editor_ui.clear_document_derived();
                    Arc::unwrap_or_clone(snapshot)
                        .exchange(self)
                        .map_err(CommandEditError::Scoped)?;
                    self.clear_document_drafts(captured.active_theme);
                    self.codegen.reset_for_document_replacement();
                    self.instance_write_virtual_anchor = None;
                    jian_ops_schema::image_thumbs::activate_for_document(&self.doc);
                }
                Ok(())
            }
        };
        if result.is_ok() {
            if let Some(actor) = batch_actor {
                actor.restore(self);
            }
        }
        result
    }
}

#[cfg(test)]
#[path = "command_capture_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "target_capture_tests.rs"]
mod target_tests;

/// Reuse the strict physical target/splice proof before an atomic write.
pub(crate) fn incremental_atomic_targets(
    state: &EditorState,
    command: &crate::EditorCommand,
) -> Option<Vec<crate::history::TargetSpec>> {
    target_plan::plan(state, command)
}
