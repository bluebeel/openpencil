//! Headless native Boolean computation, committed by the ordinary MCP applier.

use std::{collections::BTreeMap, fmt};

use op_editor_core::{BooleanOp, BooleanPathReplacement, EditorCommand, EditorState};
use op_mcp::{McpTool, ToolErrorCode, ToolOutcome};
use serde_json::json;

const MAX_SVG_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
enum BooleanError {
    Operation,
    Selection {
        count: usize,
    },
    NoResult,
    ResultLimit {
        bytes: usize,
    },
    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    )))]
    UnsupportedPlatform,
}
impl fmt::Display for BooleanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Operation => f.write_str("operation must be union, subtract, intersect or exclude"),
            Self::Selection { count } => write!(f, "Boolean operations require 2..512 unique editable canonical leaf shapes on the active page; selection has {count} operands"),
            Self::NoResult => f.write_str("native Boolean operation produced no non-empty supported result; operands were unchanged"),
            Self::ResultLimit { bytes } => write!(f, "Boolean SVG result has {bytes} bytes, exceeding the {MAX_SVG_BYTES} byte limit"),
            #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows", target_os = "ios", target_os = "android")))]
            Self::UnsupportedPlatform => f.write_str("native Boolean path operations are unavailable on this platform"),
        }
    }
}
impl std::error::Error for BooleanError {}

fn operation(args: Option<&BTreeMap<String, String>>) -> Result<BooleanOp, BooleanError> {
    match args
        .and_then(|args| args.get("operation"))
        .map(String::as_str)
    {
        Some("union") => Ok(BooleanOp::Union),
        Some("subtract") => Ok(BooleanOp::Subtract),
        Some("intersect") => Ok(BooleanOp::Intersect),
        Some("exclude") => Ok(BooleanOp::Exclude),
        _ => Err(BooleanError::Operation),
    }
}

pub(super) struct BooleanSelected {
    result: Result<(BooleanOp, BooleanPathReplacement), BooleanError>,
}
impl McpTool for BooleanSelected {
    fn name(&self) -> &str {
        "boolean_selected"
    }

    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        match &self.result {
            Ok((prepared_operation, value))
                if operation(Some(args)).ok() == Some(*prepared_operation) =>
            {
                let result = json!({
                    "operation": args["operation"],
                    "sourceIds": value.source_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
                    "curvePreserving": true,
                    "resultSelected": true,
                    "wrote": true,
                });
                ToolOutcome::OkJsonWithCommand(
                    result.to_string(),
                    EditorCommand::ReplacePathsWithSvg(Box::new(value.clone())),
                )
            }
            Ok(_) => ToolOutcome::Err(
                ToolErrorCode::InvalidArgument,
                BooleanError::Operation.to_string(),
            ),
            Err(error) => ToolOutcome::Err(
                if matches!(
                    error,
                    BooleanError::Operation | BooleanError::Selection { .. }
                ) {
                    ToolErrorCode::InvalidArgument
                } else {
                    ToolErrorCode::ToolFailed
                },
                error.to_string(),
            ),
        }
    }
}

pub(super) fn snapshot(
    state: &EditorState,
    args: Option<&BTreeMap<String, String>>,
) -> BooleanSelected {
    snapshot_inner(state, args, false)
}

pub(super) fn snapshot_cached(
    state: &EditorState,
    args: Option<&BTreeMap<String, String>>,
) -> BooleanSelected {
    snapshot_inner(state, args, true)
}

fn snapshot_inner(
    state: &EditorState,
    args: Option<&BTreeMap<String, String>>,
    trusted_authority: bool,
) -> BooleanSelected {
    let result = (|| {
        // Discovery and invalid selection never derive a layout scene.
        let operation = operation(args)?;
        let ids = &state.selection.set;
        if !state.boolean_operands_are_editable(ids) {
            return Err(BooleanError::Selection { count: ids.len() });
        }
        prepare(state, operation, trusted_authority)
    })();
    BooleanSelected { result }
}

#[cfg(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
fn prepare(
    state: &EditorState,
    operation: BooleanOp,
    trusted_authority: bool,
) -> Result<(BooleanOp, BooleanPathReplacement), BooleanError> {
    let scene = if trusted_authority {
        op_pen_loader::active_query_scene_cached(state)
    } else {
        std::sync::Arc::new(op_pen_loader::editor_state_to_active_page_layout_scene(
            state,
        ))
    };
    let selected = state
        .selection
        .set
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect::<Vec<_>>();
    let result = op_host_native::boolean_ops::compute_boolean_svg(&scene, &selected, operation)
        .ok_or(BooleanError::NoResult)?;
    if result.d.len() > MAX_SVG_BYTES {
        return Err(BooleanError::ResultLimit {
            bytes: result.d.len(),
        });
    }
    Ok((
        operation,
        BooleanPathReplacement {
            source_ids: state.selection.set.clone(),
            d: result.d,
            bounds: result.bounds,
            document_generation: state.document_generation(),
            document_revision: state.document_revision(),
            active_page_index: state.ui.active_page_index,
        },
    ))
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
)))]
fn prepare(
    _: &EditorState,
    _: BooleanOp,
    _: bool,
) -> Result<(BooleanOp, BooleanPathReplacement), BooleanError> {
    Err(BooleanError::UnsupportedPlatform)
}
