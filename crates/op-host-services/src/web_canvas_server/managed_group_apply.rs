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
    tool: &str,
    input: usize,
    budget: &mut budget::Budget,
    metrics: &metrics::Metrics,
    capture: &mut Option<CommandEditCapture>,
) -> Outcome {
    if matches!(command, EditorCommand::SetInstanceComponent { .. }) {
        let mut attempted = false;
        let result =
            editor.try_apply_captured_ref_target(command.clone(), capture, |state, bytes| {
                attempted = true;
                let reserved = budget.reserve_auxiliary(state, command, input, bytes, false);
                metrics.observe_forecast(budget.forecast());
                if let Err(reason) = reserved {
                    metrics.refuse(reason);
                    false
                } else {
                    true
                }
            });
        return result.map_or_else(
            || {
                if !attempted {
                    metrics
                        .strict_capture_fallback
                        .fetch_add(1, Ordering::Relaxed);
                }
                Outcome::Isolated
            },
            Outcome::Applied,
        );
    }
    if matches!(
        command,
        EditorCommand::AddPage { .. }
            | EditorCommand::DuplicatePage { .. }
            | EditorCommand::DeletePage { .. }
            | EditorCommand::ReorderPage { .. }
    ) {
        let mut attempted = false;
        let result = editor.try_apply_captured_pages(command.clone(), capture, |state, bytes| {
            attempted = true;
            let reserved = budget.reserve_pages(state, command, input, bytes);
            metrics.observe_forecast(budget.forecast());
            if let Err(reason) = reserved {
                metrics.refuse(reason);
                false
            } else {
                true
            }
        });
        return match result {
            Some(applied) => Outcome::Applied(applied),
            None => {
                if !attempted {
                    metrics
                        .strict_capture_fallback
                        .fetch_add(1, Ordering::Relaxed);
                }
                Outcome::Isolated
            }
        };
    }
    if matches!(command, EditorCommand::Undo | EditorCommand::Redo) {
        let mut attempted = false;
        let result = editor.try_apply_captured_history(command.clone(), capture, |state, bytes| {
            attempted = true;
            let reserved = budget.reserve_history(state, command, input, bytes);
            metrics.observe_forecast(budget.forecast());
            if let Err(reason) = reserved {
                metrics.refuse(reason);
                false
            } else {
                true
            }
        });
        return match result {
            Some(applied) => Outcome::Applied(applied),
            None => {
                if !attempted {
                    metrics
                        .strict_capture_fallback
                        .fetch_add(1, Ordering::Relaxed);
                }
                Outcome::Isolated
            }
        };
    }
    if let Some(result) = crate::mcp_serve::cohort_generation::apply(
        editor,
        command,
        tool,
        capture,
        |state, extra| {
            let reserved = budget.reserve_generated(state, command, input, extra);
            metrics.observe_forecast(budget.forecast());
            if let Err(reason) = reserved {
                metrics.refuse(reason);
                false
            } else {
                true
            }
        },
    ) {
        return match result {
            crate::mcp_serve::cohort_generation::Outcome::Applied(ok) => Outcome::Applied(ok),
            crate::mcp_serve::cohort_generation::Outcome::Isolated => Outcome::Isolated,
        };
    }
    let mut attempted = false;
    let mut admitted = false;
    if let Some(applied) =
        editor.try_apply_captured_auxiliary(command.clone(), capture, |state, bytes, components| {
            attempted = true;
            match budget.reserve_auxiliary(state, command, input, bytes, components) {
                Ok(()) => {
                    admitted = true;
                    metrics.observe_forecast(budget.forecast());
                    true
                }
                Err(reason) => {
                    metrics.observe_forecast(budget.forecast());
                    metrics.refuse(reason);
                    false
                }
            }
        })
    {
        return Outcome::Applied(applied);
    }
    if attempted {
        if admitted {
            metrics
                .strict_capture_fallback
                .fetch_add(1, Ordering::Relaxed);
        }
        return Outcome::Isolated;
    }
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
    Outcome::Applied(
        editor.apply_command_edit_without_document_repairs(
            command.clone(),
            capture.as_mut().unwrap(),
        ),
    )
}
