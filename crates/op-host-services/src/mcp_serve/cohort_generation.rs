//! Actual existing generation cleanup inside an admitted append inverse. The
//! ordinary broad path stays available for unsupported/unbounded commands.
use super::{cohort_coverage, write_repairs::RepairPlan};
use op_editor_core::{CommandEditCapture, EditorCommand, EditorState};
pub(crate) enum Outcome {
    Applied(bool),
    Isolated,
}
pub(crate) fn apply(
    editor: &mut EditorState,
    command: &EditorCommand,
    tool: &str,
    capture: &mut Option<CommandEditCapture>,
    reserve: impl FnOnce(&EditorState, usize) -> bool,
) -> Option<Outcome> {
    if cohort_coverage::repair_free(editor, tool, command) {
        return None;
    }
    let Some(extra) = editor.command_repair_capture_bound(command) else {
        return Some(Outcome::Isolated);
    };
    if !reserve(editor, extra) {
        return Some(Outcome::Isolated);
    }
    let Some(checkpoint) = editor.try_begin_command_edit_without_document_repairs(command) else {
        return Some(Outcome::Isolated);
    };
    let repairs = RepairPlan::capture(editor, command, true);
    let before = editor.document_revision();
    *capture = Some(checkpoint);
    Some(Outcome::Applied(editor.apply_command_edit_then(
        command.clone(),
        capture.as_mut().unwrap(),
        move |state| {
            repairs.apply(state, before);
        },
    )))
}
