//! Fresh, admitted selective-history exchange. The reverse snapshot owns the
//! exact former live payload; the History checkpoint shares immutable graphs.
use super::{CaptureKind, CommandEditCapture, EditorCommand, EditorState, HistoryCapture};

impl EditorState {
    /// No legacy/Full replay is represented as a bounded receipt. Empty,
    /// conflicted, broad or unsupported history stays on ordinary dispatch.
    pub fn forecast_history_command_edit(
        &self,
        command: &EditorCommand,
        limit: usize,
    ) -> Option<usize> {
        let redo = match command {
            EditorCommand::Undo => false,
            EditorCommand::Redo => true,
            _ => return None,
        };
        if self.editor_ui.workspace.active || self.undo_capture_suspended {
            return None;
        }
        self.history.validate_actor(redo).ok()?;
        let entry = self.history.next_entry(redo)?;
        if !self.history.selective_exchange(entry) {
            return None;
        }
        entry.scoped_snapshot()?.cohort_exchange_bound(self, limit)
    }
    /// One locked actor authority, actual validated history, one admitted
    /// inverse. The immutable admission callback cannot mutate the editor;
    /// there is no replay or public revision-based cache between proof/apply.
    pub fn try_apply_captured_history(
        &mut self,
        command: EditorCommand,
        capture: &mut Option<CommandEditCapture>,
        admit: impl FnOnce(&Self, usize) -> bool,
    ) -> Option<bool> {
        if capture.is_some() {
            return None;
        }
        let bytes = self.forecast_history_command_edit(
            &command,
            crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE,
        )?;
        if !admit(self, bytes) {
            return None;
        }
        self.invalidate_command_id_sequence();
        let undo = matches!(command, EditorCommand::Undo);
        *capture = Some(CommandEditCapture {
            rollback_clipboard: None,
            batch_actor: None,
            kind: CaptureKind::History(HistoryCapture {
                actor_selective: true,
                generation: self.document_generation,
                revision: self.revision,
                undo,
                history: self.history.clone(),
                completed: None,
                started: false,
                active_theme: self.ui.variables.active_theme.clone(),
            }),
        });
        let CaptureKind::History(checkpoint) = &mut capture.as_mut().unwrap().kind else {
            unreachable!()
        };
        // Fresh borrowed forecast -> immutable admission -> checkpoint -> this
        // exact exchange. Rechecking all History values structurally here would
        // duplicate the entire retained graph walk for every Undo request.
        checkpoint.started = true;
        let (applied, reverse) = self.exchange_history_retained(!undo);
        checkpoint.completed = reverse;
        checkpoint.started = applied && checkpoint.completed.is_none();
        if checkpoint.started {
            // Impossible for the scoped entry proved above, but do not turn
            // an absent inverse into a durable acknowledgement if that contract
            // ever changes. Host abort sees the incomplete receipt and fences.
            return Some(false);
        }
        if !applied {
            // Normal failed exchange mutates no canonical data. Keep its exact
            // actor-history error/runtime validation result for the real host
            // wrapper, rather than overwriting it with the pre-call checkpoint.
            *capture = None;
        }
        Some(applied)
    }
}
