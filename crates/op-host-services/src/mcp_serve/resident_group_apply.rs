//! Admit complete structural/auxiliary captures before any owned clone. This is
//! the same strict Core path as primary cohorts, with the resident global quota.
use super::super::{budget, metrics, quota};
use op_editor_core::{CommandEditCapture, EditorCommand, EditorState};
use std::sync::atomic::Ordering;
pub(super) enum Outcome {
    Applied(bool),
    Isolated,
}
fn reserve(
    budget: &budget::Budget,
    quota: &mut quota::Reservation,
    metrics: &metrics::Metrics,
    ok: bool,
) -> bool {
    if !ok || !quota.grow_to(budget.forecast().projected) {
        metrics.capture_refusals.fetch_add(1, Ordering::Relaxed);
        false
    } else {
        true
    }
}
pub(super) fn apply(
    editor: &mut EditorState,
    command: &EditorCommand,
    tool: &str,
    input: usize,
    budget: &mut budget::Budget,
    quota: &mut quota::Reservation,
    metrics: &metrics::Metrics,
    capture: &mut Option<CommandEditCapture>,
) -> Outcome {
    if matches!(command, EditorCommand::Undo | EditorCommand::Redo) {
        let result = editor.try_apply_captured_history(command.clone(), capture, |state, bytes| {
            let ok = budget.reserve_history(state, command, input, bytes).is_ok();
            reserve(budget, quota, metrics, ok)
        });
        return result.map_or(Outcome::Isolated, Outcome::Applied);
    }
    if let Some(result) =
        crate::mcp_serve::cohort_generation::apply(editor, command, tool, capture, |state, extra| {
            let ok = budget
                .reserve_generated(state, command, input, extra)
                .is_ok();
            reserve(budget, quota, metrics, ok)
        })
    {
        return match result {
            crate::mcp_serve::cohort_generation::Outcome::Applied(ok) => Outcome::Applied(ok),
            crate::mcp_serve::cohort_generation::Outcome::Isolated => Outcome::Isolated,
        };
    }
    let mut attempted = false;
    if let Some(applied) =
        editor.try_apply_captured_auxiliary(command.clone(), capture, |state, bytes, components| {
            attempted = true;
            let ok = budget
                .reserve_auxiliary(state, command, input, bytes, components)
                .is_ok();
            reserve(budget, quota, metrics, ok)
        })
    {
        return Outcome::Applied(applied);
    }
    if attempted {
        return Outcome::Isolated;
    }
    if let Some(applied) =
        editor.try_apply_captured_metadata(command.clone(), capture, |state, target| {
            attempted = true;
            let ok = budget
                .reserve_metadata_target(state, command, input, target)
                .is_ok();
            reserve(budget, quota, metrics, ok)
        })
    {
        return Outcome::Applied(applied);
    }
    if attempted {
        return Outcome::Isolated;
    }
    let ok = budget.reserve(editor, command, input).is_ok();
    if !reserve(budget, quota, metrics, ok) {
        return Outcome::Isolated;
    }
    let Some(checkpoint) = editor.try_begin_command_edit_preserving_components(command) else {
        return Outcome::Isolated;
    };
    debug_assert!(checkpoint.is_target_only());
    *capture = Some(checkpoint);
    Outcome::Applied(
        editor.apply_command_edit_without_document_repairs(
            command.clone(),
            capture.as_mut().unwrap(),
        ),
    )
}
