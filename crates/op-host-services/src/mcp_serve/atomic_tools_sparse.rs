//! Exact scalar-only preparation without a whole-document simulation.
//! Each allowed factory is an actual stateless MCP setter. Produced commands
//! must independently pass Core's sparse semantic classifier before use.

use std::collections::BTreeMap;

use op_editor_core::{
    validate_sparse_atomic_commands, EditorCommand, EditorState, SparseAtomicValidation,
};
use op_mcp::{ToolErrorCode, ToolOutcome};
use serde_json::{json, Value};

use super::{input, BatchError, Result, MAX_COMMANDS, MAX_RESULT_BYTES};
use crate::mcp_serve::tool_profile::McpAccessProfile;

pub(super) fn prepare(
    source: &EditorState,
    steps: &[input::Step],
    arguments: &BTreeMap<String, String>,
    profile: McpAccessProfile,
) -> Result<Option<ToolOutcome>> {
    if !steps.iter().all(|step| stateless_setter(&step.name)) {
        return Ok(None);
    }
    let mut commands = Vec::new();
    let mut command_steps = Vec::new();
    let mut results = Vec::new();
    let mut bindings = BTreeMap::new();
    let mut argument_bytes = 0usize;
    let mut result_bytes = 0usize;
    let mut pending_error = None;
    for (index, step) in steps.iter().enumerate() {
        let collected = collect(
            source,
            step,
            index,
            &results,
            &bindings,
            profile,
            &mut argument_bytes,
        );
        let (result, members) = match collected {
            Ok(result) => result,
            Err(error) => {
                pending_error = Some(error.at(index));
                break;
            }
        };
        if commands.len().saturating_add(members.len()) > MAX_COMMANDS {
            pending_error = Some(BatchError::argument("batch exceeds 1024 commands").at(index));
            break;
        }
        command_steps.extend(std::iter::repeat_n(index, members.len()));
        commands.extend(members);
        // These commands never select, allocate or change pages. The complete
        // source selection and the real subtool result are exact, not a fake
        // partial-document read. Core verifies that command assumption below.
        let result = json!({"index":index,"name":step.name,"result":result,
            "newIds":[],"selectedIds":source.selection.set.iter()
                .map(|id| id.as_str()).collect::<Vec<_>>()});
        result_bytes = result_bytes.saturating_add(result.to_string().len());
        if result_bytes > MAX_RESULT_BYTES {
            pending_error = Some(BatchError::argument("step results exceed 4 MiB").at(index));
            break;
        }
        if let Some(binding) = &step.bind {
            bindings.insert(binding.clone(), index);
        }
        results.push(result);
    }
    let document_changed = match validate_sparse_atomic_commands(source, &commands) {
        SparseAtomicValidation::Unsupported => return Ok(None),
        SparseAtomicValidation::Rejected { index } => {
            return Err(BatchError::new(
                ToolErrorCode::ToolFailed,
                "subtool command rejected during isolated validation",
            )
            .at(command_steps[index]));
        }
        SparseAtomicValidation::Accepted { document_changed } => document_changed,
    };
    // Preserve the original first-error order: a prior command's kind/range
    // rejection wins over a later argument/result-limit error. Constructors
    // above are pure stateless validators; no live operation was replayed.
    if let Some(error) = pending_error {
        return Err(error);
    }
    if arguments.contains_key("agentId") {
        crate::mcp_serve::agent_context::validate_atomic_ui(source)
            .map_err(|error| BatchError::new(ToolErrorCode::ToolFailed, error.to_string()))?;
    }
    let result = json!({"atomic":true,"operationCount":results.len(),
        "commandCount":commands.len(),"documentChanged":document_changed,
        "newIdsOrder":"document-preorder","results":results});
    Ok(Some(ToolOutcome::OkJsonWithCommand(
        result.to_string(),
        EditorCommand::Batch { commands },
    )))
}

fn collect(
    source: &EditorState,
    step: &input::Step,
    index: usize,
    results: &[Value],
    bindings: &BTreeMap<String, usize>,
    profile: McpAccessProfile,
    argument_bytes: &mut usize,
) -> Result<(Value, Vec<EditorCommand>)> {
    let value = input::resolve(step.arguments.clone(), results, bindings)?;
    let call = input::call(index, &step.name, value, argument_bytes)?;
    if let Some(refusal) = profile.refuse_call(&call.tool, &call.arguments) {
        return Err(BatchError::new(
            ToolErrorCode::ToolFailed,
            refusal.message(&call.tool),
        ));
    }
    input::one_authority(&call.arguments)?;
    // Each listed factory is state-independent; use the real source registry
    // rather than invent a partial EditorState for validation or discovery.
    let registry = crate::mcp_serve::registry::rebuild_registry(
        source,
        Some(&call.tool),
        profile,
        Some(&call.arguments),
    );
    let (result, command) = super::outcome(registry.dispatch_once(call), false)?;
    let command =
        command.ok_or_else(|| BatchError::argument("authoring tool must return a command"))?;
    Ok((result, super::flatten(command)?))
}

fn stateless_setter(name: &str) -> bool {
    matches!(
        name,
        "update_node"
            | "set_node_rotation"
            | "set_node_text"
            | "set_node_corner_radius"
            | "set_node_font_size"
            | "set_node_font_weight"
            | "set_node_stroke_hex"
            | "set_node_stroke_width"
            | "set_node_stroke_side_width"
            | "set_node_fill_hex"
            | "set_node_name"
            | "set_node_hidden"
            | "set_node_locked"
            | "set_node_collapsed"
            | "set_node_flip"
            | "add_node_effect"
            | "remove_node_effect"
    )
}
