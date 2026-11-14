//! Trusted command transactions with compact page-local rollback/history.
use super::*;
use crate::{
    history::PendingScopedSnapshot, EditorCommand, HistoryEntry, ScopedEditorSnapshot,
    ScopedHistoryError,
};
use std::{mem, sync::Arc};
#[path = "command_scope.rs"]
mod command_scope;

#[derive(Debug)]
pub struct CommandEditCapture {
    kind: CaptureKind,
}
#[derive(Debug)]
enum CaptureKind {
    Full(LocalEditCapture),
    Refused { generation: u64 },
    Scoped(ScopedCapture),
}
#[derive(Debug)]
struct ScopedCapture {
    generation: u64,
    revision: u64,
    pending: Option<PendingScopedSnapshot>,
    completed: Option<Arc<ScopedEditorSnapshot>>,
    history: History,
    active_theme: BTreeMap<String, String>,
}
#[derive(Debug)]
pub enum CommandEditError {
    Full(LocalEditError),
    Scoped(ScopedHistoryError),
}
impl std::fmt::Display for CommandEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full(error) => write!(f, "{error}"),
            Self::Scoped(error) => write!(f, "{error}"),
        }
    }
}
impl std::error::Error for CommandEditError {}
impl CommandEditCapture {
    pub fn is_scoped(&self) -> bool {
        matches!(self.kind, CaptureKind::Scoped(_))
    }
}
impl EditorState {
    /// Capture before an exclusively owned host command. The caller guarantees
    /// synchronous maintenance changes only the original active page; known
    /// commands also capture their explicitly requested page. Audited metadata commands capture exact
    /// header fields too; unclassified global
    /// commands keep the complete existing LocalEditCapture path. Public raw
    /// document writers and arbitrary callbacks must keep begin_local_edit.
    pub fn begin_command_edit(&self, command: &EditorCommand) -> CommandEditCapture {
        let kind = match command_scope::classify(self, command) {
            command_scope::Classification::Scoped(plan) => CaptureKind::Scoped(ScopedCapture {
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
        CommandEditCapture { kind }
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
        match &mut capture.kind {
            CaptureKind::Refused { .. } => false,
            CaptureKind::Full(full) => self.apply_undoable_captured_then(command, full, maintain),
            CaptureKind::Scoped(captured) => {
                if captured.generation != self.document_generation
                    || captured.revision != self.revision
                    || captured.pending.is_none()
                {
                    return false;
                }
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
                let mut pending = captured.pending.take().unwrap();
                let undo_redo = matches!(command, EditorCommand::Undo | EditorCommand::Redo);
                let history = (!undo_redo).then(|| mem::take(&mut self.history));
                if !undo_redo {
                    self.undo_capture_suspended = true;
                }
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let applied = if let EditorCommand::Batch { commands } = command {
                        let batch = EditorCommand::Batch { commands };
                        let mut allocator =
                            if crate::command_apply_legacy::allocation_policy::requires_id_seed(
                                &batch,
                            ) {
                                let Ok(allocator) =
                                    crate::DocumentIdAllocator::sequential_for_document(&self.doc)
                                else {
                                    return false;
                                };
                                allocator
                            } else {
                                crate::DocumentIdAllocator::sequential(1)
                            };
                        let EditorCommand::Batch { commands } = batch else {
                            unreachable!()
                        };
                        self.cmd_batch_with_scoped_checkpoint(
                            commands,
                            &mut allocator,
                            &mut pending,
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
    /// Restore document, history and derived UI after external persistence
    /// rejects a trusted command. Clipboard remains the host's moved rollback
    /// responsibility, matching abort_local_edit.
    pub fn abort_command_edit(
        &mut self,
        capture: CommandEditCapture,
    ) -> Result<(), CommandEditError> {
        match capture.kind {
            CaptureKind::Refused { generation } => self
                .ensure_local_edit_generation(generation)
                .map_err(CommandEditError::Full),
            CaptureKind::Full(full) => self.abort_local_edit(full).map_err(CommandEditError::Full),
            CaptureKind::Scoped(captured) => {
                self.ensure_local_edit_generation(captured.generation)
                    .map_err(CommandEditError::Full)?;
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
        }
    }
}

#[cfg(test)]
#[path = "command_capture_tests.rs"]
mod tests;
