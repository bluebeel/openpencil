//! Lexical exact header/name transactions for synchronous no-repair hosts.
use super::*;
impl EditorState {
    /// Payload bytes and whether a complete component checkpoint is retained.
    pub fn forecast_auxiliary_command_edit(
        &self,
        command: &EditorCommand,
    ) -> Option<(usize, bool)> {
        crate::history::auxiliary_command_forecast(self, command)
    }
    pub(super) fn prepare_auxiliary_command_edit(
        &self,
        command: &EditorCommand,
    ) -> Option<CommandEditCapture> {
        let pending = PendingScopedSnapshot::capture_auxiliary(self, command)?;
        Some(CommandEditCapture {
            kind: CaptureKind::Scoped(ScopedCapture {
                targeted: true,
                generation: self.document_generation,
                revision: self.revision,
                pending: Some(pending),
                completed: None,
                history: self.history.clone(),
                active_theme: self.ui.variables.active_theme.clone(),
            }),
            batch_actor: None,
            rollback_clipboard: None,
        })
    }
    /// Known key/name writers own only their audited fields. Admission borrows
    /// state before payload capture, and the slot owns rollback before mutation.
    pub fn try_apply_captured_auxiliary(
        &mut self,
        command: EditorCommand,
        capture: &mut Option<CommandEditCapture>,
        admit: impl FnOnce(&Self, usize, bool) -> bool,
    ) -> Option<bool> {
        self.observe_command_id_sequence(&command);
        if capture.is_some() {
            return None;
        }
        let (bytes, components) = self.forecast_auxiliary_command_edit(&command)?;
        if !admit(self, bytes, components) {
            return None;
        }
        *capture = Some(self.prepare_auxiliary_command_edit(&command)?);
        Some(self.apply_command_edit_without_document_repairs(command, capture.as_mut().unwrap()))
    }
}
