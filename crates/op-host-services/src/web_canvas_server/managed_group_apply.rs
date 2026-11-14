//! Reuse one request's resolved target across forecast, capture and mutation.
use super::{budget, metrics};
use op_editor_core::{CommandEditCapture, EditorCommand, EditorState};
use std::sync::atomic::Ordering;
pub(super) enum Outcome {
    Applied(bool),
    Isolated,
}
pub(super) fn apply(
    editor: &mut EditorState,
    command: &EditorCommand,
    input: usize,
    budget: &mut budget::Budget,
    metrics: &metrics::Metrics,
    capture: &mut Option<CommandEditCapture>,
) -> Outcome {
    let mut attempted = false;
    let mut admitted = false;
    if let Some(applied) =
        editor.try_apply_captured_metadata(command.clone(), capture, |state, target| {
            attempted = true;
            let reserved = budget.reserve_metadata_target(state, command, input, target);
            metrics.observe_forecast(budget.forecast());
            if let Err(reason) = reserved {
                metrics.refuse(reason);
                false
            } else {
                admitted = true;
                true
            }
        })
    {
        return Outcome::Applied(applied);
    }
    // Admission/capture refusal has applied nothing. Do not charge twice or
    // continue with another target after a prepared request became ineligible.
    if attempted {
        if admitted {
            metrics
                .strict_capture_fallback
                .fetch_add(1, Ordering::Relaxed);
        }
        return Outcome::Isolated;
    }
    let reserved = budget.reserve(editor, command, input);
    metrics.observe_forecast(budget.forecast());
    if let Err(reason) = reserved {
        metrics.refuse(reason);
        return Outcome::Isolated;
    }
    let Some(checkpoint) = editor.try_begin_command_edit_preserving_components(command) else {
        metrics
            .strict_capture_fallback
            .fetch_add(1, Ordering::Relaxed);
        return Outcome::Isolated;
    };
    debug_assert!(checkpoint.is_target_only());
    *capture = Some(checkpoint);
    Outcome::Applied(editor.apply_command_edit_then(
        command.clone(),
        capture.as_mut().unwrap(),
        |_| {},
    ))
}
