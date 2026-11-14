//! One-call host adapter: compute against a borrowed editor, then transfer the
//! owned outcome after that borrow ends. Large commands are moved, not cloned.

use std::{collections::BTreeMap, sync::Mutex};

use op_editor_core::EditorState;

use crate::{McpTool, ToolErrorCode, ToolOutcome};

struct PreparedBatchDesign {
    arguments: BTreeMap<String, String>,
    outcome: Mutex<Option<ToolOutcome>>,
}

/// Prepare exactly one `batch_design` call for a one-request host registry.
/// Mixed programs clone the editor once for isolated simulation. The legacy
/// reusable snapshot constructors retain their existing behavior.
pub fn prepare_batch_design_call(
    state: &EditorState,
    arguments: &BTreeMap<String, String>,
) -> Box<dyn McpTool> {
    // The common scalar-node insert path already needs no editor copy. Keep
    // its existing cheap adapter instead of staging and locking an outcome.
    if matches!(
        super::batch_design::select_batch_input(arguments),
        Ok(super::batch_design::BatchInputKind::NodesJson)
    ) {
        return Box::new(super::batch_design_snapshot_for_call(state, arguments));
    }
    Box::new(PreparedBatchDesign {
        arguments: arguments.clone(),
        outcome: Mutex::new(Some(super::batch_design_result::outcome_for_call(
            state, arguments,
        ))),
    })
}

impl McpTool for PreparedBatchDesign {
    fn name(&self) -> &str {
        "batch_design"
    }

    fn call(&self, arguments: &BTreeMap<String, String>) -> ToolOutcome {
        if arguments != &self.arguments {
            return ToolOutcome::Err(
                ToolErrorCode::InvalidArgument,
                "batch_design request-scoped snapshot cannot run different arguments".into(),
            );
        }
        match self.outcome.lock() {
            Ok(mut outcome) => outcome.take().unwrap_or_else(|| {
                ToolOutcome::Err(
                    ToolErrorCode::ToolFailed,
                    "prepared batch_design call was already consumed".into(),
                )
            }),
            Err(_) => ToolOutcome::Err(
                ToolErrorCode::Internal,
                "prepared batch_design call is unavailable".into(),
            ),
        }
    }
}

#[cfg(test)]
#[path = "batch_design_prepared_tests.rs"]
mod tests;
