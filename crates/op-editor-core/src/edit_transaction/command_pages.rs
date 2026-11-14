//! Fresh page-list capture: one exact page delta, no full-document checkpoint.
use super::{
    CaptureKind, CommandEditCapture, EditorCommand, EditorState, ScopedCapture,
    ScopedEditorSnapshot,
};
impl EditorState {
    pub fn forecast_page_command_edit(
        &self,
        command: &EditorCommand,
        limit: usize,
    ) -> Option<usize> {
        ScopedEditorSnapshot::page_command_forecast(self, command, limit).map(|bound| bound.bytes())
    }
    pub fn try_apply_captured_pages(
        &mut self,
        command: EditorCommand,
        capture: &mut Option<CommandEditCapture>,
        admit: impl FnOnce(&Self, usize) -> bool,
    ) -> Option<bool> {
        if capture.is_some()
            || !matches!(
                command,
                EditorCommand::AddPage { .. }
                    | EditorCommand::DuplicatePage { .. }
                    | EditorCommand::DeletePage { .. }
                    | EditorCommand::ReorderPage { .. }
            )
        {
            return None;
        }
        self.invalidate_command_id_sequence();
        let bound = ScopedEditorSnapshot::page_command_forecast(
            self,
            &command,
            crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE,
        )?;
        if !admit(self, bound.bytes()) {
            return None;
        }
        let plan = ScopedEditorSnapshot::prepare_page_command(self, command, bound)?;
        *capture = Some(CommandEditCapture {
            rollback_clipboard: None,
            batch_actor: None,
            kind: CaptureKind::Scoped(ScopedCapture {
                targeted: true,
                generation: self.document_generation(),
                revision: self.document_revision(),
                pending: None,
                completed: None,
                history: self.history.clone(),
                active_theme: self.ui.variables.active_theme.clone(),
            }),
        });
        // An unexpected panic after the first write leaves an incomplete
        // capture, which existing abort/fencing refuses to call a clean rollback.
        let snapshot = plan.commit(self);
        self.history_push_entry(crate::HistoryEntry::Scoped(Arc::clone(&snapshot)));
        let CaptureKind::Scoped(checkpoint) = &mut capture.as_mut().unwrap().kind else {
            unreachable!()
        };
        checkpoint.completed = Some(snapshot);
        Some(true)
    }
}
use std::sync::Arc;
