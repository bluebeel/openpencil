//! One undo boundary around a host command and its synchronous design repairs.

use super::write_repairs::RepairPlan;
use op_editor_core::{CommandEditCapture, CommandEditError, EditorCommand, EditorState};

/// Keep the repair proof and capture together: ordinary calls can capture only
/// exact targets, while an actual generation repair retains the broad contract.
pub(crate) struct PreparedCommandEdit {
    capture: CommandEditCapture,
    repairs: Option<RepairPlan>,
}
impl PreparedCommandEdit {
    pub(crate) fn owns_replaced_clipboard_rollback(&self) -> bool {
        self.capture.owns_replaced_clipboard_rollback()
    }
    pub(crate) fn visit_metadata_changes(
        &self,
        state: &EditorState,
        visitor: impl FnMut(&jian_ops_schema::node::PenNode, &jian_ops_schema::node::PenNode),
    ) -> bool {
        self.capture.visit_metadata_changes(state, visitor)
    }

    pub(crate) fn visit_canonical_deltas<'a>(
        &'a self,
        state: &'a EditorState,
        limit: usize,
        nodes: impl FnMut(&jian_ops_schema::node::PenNode, &jian_ops_schema::node::PenNode),
        headers: impl FnMut(op_editor_core::CanonicalHeaderChange) -> bool,
        appends: impl FnMut(op_editor_core::CanonicalSplice<'a>) -> bool,
    ) -> bool {
        self.capture
            .visit_canonical_deltas(state, limit, nodes, headers, appends)
    }

    pub(crate) fn forward_metadata_json(
        &self,
        state: &EditorState,
        limit: usize,
    ) -> Option<Vec<u8>> {
        self.capture.forward_metadata_json(state, limit)
    }

    pub(crate) fn abort(self, state: &mut EditorState) -> Result<(), CommandEditError> {
        state.abort_command_edit(self.capture)
    }
}

pub(crate) fn begin_command_capture(
    state: &EditorState,
    command: &EditorCommand,
    repair: bool,
) -> PreparedCommandEdit {
    let repairs = RepairPlan::capture(state, command, repair);
    let capture = if repairs.is_empty() {
        state.begin_command_edit_without_document_repairs(command)
    } else {
        state.begin_command_edit(command)
    };
    PreparedCommandEdit {
        capture,
        repairs: Some(repairs),
    }
}

/// Production synchronous paths transfer Cut's prior actor clipboard into
/// its checkpoint. The immutable constructor remains available to callers that
/// only prepare a checkpoint without immediately applying it. Proven ordinary
/// header edits may also leave the document-backed catalog untouched.
pub(crate) fn begin_command_capture_moving_clipboard(
    state: &mut EditorState,
    command: &EditorCommand,
    repair: bool,
) -> PreparedCommandEdit {
    if repair {
        return super::begin_command_capture(state, command, repair);
    }
    let repairs = RepairPlan::capture(state, command, repair);
    let capture = if repairs.is_empty() {
        state
            .try_begin_command_edit_preserving_components(command)
            .unwrap_or_else(|| state.begin_command_edit(command))
    } else {
        state.begin_command_edit(command)
    };
    PreparedCommandEdit {
        capture,
        repairs: Some(repairs),
    }
}

pub(crate) fn apply_command_with_undo(
    state: &mut EditorState,
    command: EditorCommand,
    repair: bool,
) -> bool {
    apply_command_with_undo_report(state, command, repair).applied
}

pub(crate) struct CommandApplyReport {
    pub applied: bool,
    pub layout_repaired: bool,
}

pub(crate) fn apply_command_with_undo_report(
    state: &mut EditorState,
    command: EditorCommand,
    repair: bool,
) -> CommandApplyReport {
    // Individual view commands never needed a document checkpoint. A UI-only
    // Batch still uses the scoped path for its atomic rollback boundary.
    if !command.may_change_document() && !matches!(command, EditorCommand::Batch { .. }) {
        let before = state.document_revision();
        let plan = RepairPlan::capture(state, &command, repair);
        let mut layout_repaired = false;
        let applied = state.apply_undoable_then(command, |state| {
            layout_repaired = plan.apply(state, before);
        });
        return CommandApplyReport {
            applied,
            layout_repaired,
        };
    }
    let mut capture = begin_command_capture_moving_clipboard(state, &command, repair);
    apply_prepared(state, command, &mut capture)
}

pub(crate) fn apply_command_with_command_capture(
    state: &mut EditorState,
    command: EditorCommand,
    capture: &mut PreparedCommandEdit,
) -> bool {
    apply_prepared(state, command, capture).applied
}

fn apply_prepared(
    state: &mut EditorState,
    command: EditorCommand,
    capture: &mut PreparedCommandEdit,
) -> CommandApplyReport {
    let Some(plan) = capture.repairs.take() else {
        return CommandApplyReport {
            applied: false,
            layout_repaired: false,
        };
    };
    let before = state.document_revision();
    let mut layout_repaired = false;
    let applied = state.apply_command_edit_then(command, &mut capture.capture, |state| {
        layout_repaired = plan.apply(state, before);
    });
    CommandApplyReport {
        applied,
        layout_repaired,
    }
}

#[cfg(test)]
#[path = "command_capture_host_tests.rs"]
mod tests;
