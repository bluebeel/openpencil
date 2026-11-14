//! One undo boundary around a host command and its synchronous design repairs.

use op_editor_core::{EditorCommand, EditorState};

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
    apply_command_impl(state, command, repair, None)
}

pub(crate) fn apply_command_with_undo_captured(
    state: &mut EditorState,
    command: EditorCommand,
    repair: bool,
    capture: &op_editor_core::LocalEditCapture,
) -> bool {
    apply_command_impl(state, command, repair, Some(capture)).applied
}

fn apply_command_impl(
    state: &mut EditorState,
    command: EditorCommand,
    repair: bool,
    capture: Option<&op_editor_core::LocalEditCapture>,
) -> CommandApplyReport {
    let before = state.document_revision();
    let mut layout_repaired = false;
    let maintain = |state: &mut EditorState| {
        if repair {
            layout_repaired = super::repair_after_document_change(state, before);
        }
    };
    let applied = match capture {
        Some(capture) => state.apply_undoable_captured_then(command, capture, maintain),
        None => state.apply_undoable_then(command, maintain),
    };
    CommandApplyReport {
        applied,
        layout_repaired,
    }
}
