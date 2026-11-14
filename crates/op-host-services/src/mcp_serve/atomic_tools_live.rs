//! Single-execution atomic authoring under an explicitly bound file authority.
//! Full source reads/ref resolution stay on the live complete EditorState.
use super::*;
use crate::mcp_serve::McpServeError;
use op_editor_core::{AtomicAuthorityEdit, AtomicAuthorityError};

enum Failure {
    Batch(BatchError),
    Legacy,
}
impl From<BatchError> for Failure {
    fn from(error: BatchError) -> Self {
        Self::Batch(error)
    }
}

pub(crate) fn try_execute<F>(
    state: &mut EditorState,
    arguments: &BTreeMap<String, String>,
    profile: McpAccessProfile,
    mut callback: F,
) -> std::result::Result<Option<Box<dyn McpTool>>, McpServeError>
where
    F: FnMut(&mut EditorState, AuthorityAction<'_>) -> bool,
{
    // Eligibility is conservative, before any constructor or live command.
    // Unknown/resource/global-state callbacks retain the complete old path.
    let Ok(steps) = input::parse(arguments) else {
        return Ok(None);
    };
    if steps
        .iter()
        .any(|step| allowed_name(&step.name, profile).is_err())
        || steps.iter().any(|step| !eligible(&step.name))
        || !steps.iter().any(|step| structural(&step.name))
    {
        return Ok(None);
    }
    let mut edit = match AtomicAuthorityEdit::begin(state) {
        Ok(edit) => Some(edit),
        Err(AtomicAuthorityError::Unsupported) => return Ok(None),
        Err(error) => return Err(McpServeError::Document(error.to_string())),
    };
    let revision = state.document_revision();
    let generation = state.document_generation();
    let page = state.ui.active_page_index;
    let preserve = state.editor_ui.preserve_authored_geometry;
    let metadata =
        (!arguments.contains_key("agentId")).then(|| op_pen_loader::EditorMeta::from_state(state));
    let mut commit_started = false;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (json, count) = execute_steps(
            state,
            edit.as_mut().unwrap(),
            steps,
            arguments,
            profile,
            &mut callback,
        )?;
        edit.as_mut().unwrap().seal(state).map_err(|error| {
            Failure::Batch(BatchError::new(
                ToolErrorCode::ToolFailed,
                error.to_string(),
            ))
        })?;
        let summary = AtomicCommit {
            previous_revision: revision,
            previous_generation: generation,
            previous_active_page: page,
            previous_preserve_geometry: preserve,
            editor_metadata_changed: metadata
                .as_ref()
                .is_some_and(|before| before != &op_pen_loader::EditorMeta::from_state(state)),
            command_count: count,
        };
        commit_started = true;
        if !callback(state, AuthorityAction::CommitAtomic(&summary)) {
            return Err(Failure::Batch(BatchError::new(
                ToolErrorCode::ToolFailed,
                "atomic persistence was refused or failed",
            )));
        }
        edit.take().unwrap().accept();
        Ok::<_, Failure>(ToolOutcome::OkJson(json))
    }));
    let outcome = match result {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(failure)) => {
            let rollback = edit
                .take()
                .ok_or(AtomicAuthorityError::IncompleteRollback)
                .and_then(|edit| edit.abort(state));
            if let Err(error) = rollback {
                return uncertain(state, &mut callback, &error.to_string());
            }
            match failure {
                Failure::Legacy if !commit_started => return Ok(None),
                Failure::Legacy => {
                    return uncertain(
                        state,
                        &mut callback,
                        "legacy retry requested after persistence began",
                    )
                }
                Failure::Batch(error) => ToolOutcome::Err(error.code, error.to_string()),
            }
        }
        Err(_) if commit_started => {
            // The callback may have renamed its file already. Do not claim
            // rollback/no-op or replay after an uncertain persistence result.
            return uncertain(
                state,
                &mut callback,
                "atomic persistence panicked; commit outcome is unknown",
            );
        }
        Err(_) => {
            let rollback = edit
                .take()
                .ok_or(AtomicAuthorityError::IncompleteRollback)
                .and_then(|edit| edit.abort(state));
            if let Err(error) = rollback {
                return uncertain(state, &mut callback, &error.to_string());
            }
            ToolOutcome::Err(
                ToolErrorCode::ToolFailed,
                "batch-tools-refused: atomic execution panicked; no operations were applied".into(),
            )
        }
    };
    Ok(Some(Box::new(Prepared {
        arguments: arguments.clone(),
        outcome: Mutex::new(Some(outcome)),
    })))
}
fn uncertain<F>(
    state: &mut EditorState,
    callback: &mut F,
    reason: &str,
) -> std::result::Result<Option<Box<dyn McpTool>>, McpServeError>
where
    F: FnMut(&mut EditorState, AuthorityAction<'_>) -> bool,
{
    callback(state, AuthorityAction::AtomicUncertain(reason));
    Err(McpServeError::Document(format!(
        "batch-tools-recovery-required: {reason}"
    )))
}

fn execute_steps<F>(
    state: &mut EditorState,
    edit: &mut AtomicAuthorityEdit,
    steps: Vec<input::Step>,
    arguments: &BTreeMap<String, String>,
    profile: McpAccessProfile,
    callback: &mut F,
) -> std::result::Result<(String, usize), Failure>
where
    F: FnMut(&mut EditorState, AuthorityAction<'_>) -> bool,
{
    let initial_revision = state.document_revision();
    let mut results = Vec::new();
    let mut bindings = BTreeMap::new();
    let mut read_cache: HashMap<(String, BTreeMap<String, String>), Value> = HashMap::new();
    let mut result_bytes = 0usize;
    let mut argument_bytes = 0usize;
    let mut count = 0usize;
    for (index, step) in steps.into_iter().enumerate() {
        let value =
            input::resolve(step.arguments, &results, &bindings).map_err(|error| error.at(index))?;
        let call = input::call(index, &step.name, value, &mut argument_bytes)
            .map_err(|error| error.at(index))?;
        if let Some(refusal) = profile.refuse_call(&call.tool, &call.arguments) {
            return Err(
                BatchError::new(ToolErrorCode::ToolFailed, refusal.message(&call.tool))
                    .at(index)
                    .into(),
            );
        }
        input::one_authority(&call.arguments).map_err(|error| error.at(index))?;
        let read =
            tool_profile::profile_for(&call.tool).is_some_and(|p| p.access == ToolAccess::Read);
        let key = (read && cacheable_document_read(&call.tool))
            .then(|| (call.tool.clone(), call.arguments.clone()));
        let (result, command) = match key.as_ref().and_then(|key| read_cache.get(key)) {
            Some(result) => (result.clone(), None),
            None => {
                let registry = state.with_trusted_read_lookup(|state| {
                    crate::mcp_serve::registry::rebuild_registry(
                        state,
                        Some(&call.tool),
                        profile,
                        Some(&call.arguments),
                    )
                });
                let outcome =
                    outcome(registry.dispatch_once(call), read).map_err(|error| error.at(index))?;
                if let Some(key) = key {
                    read_cache.insert(key, outcome.0.clone());
                }
                outcome
            }
        };
        let members = match command {
            Some(command) => flatten(command).map_err(|error| error.at(index))?,
            None => Vec::new(),
        };
        if count.saturating_add(members.len()) > MAX_COMMANDS {
            return Err(BatchError::argument("batch exceeds 1024 commands")
                .at(index)
                .into());
        }
        edit.begin_identity_step();
        for command in members {
            if !command.supports_atomic_batch() {
                return Err(Failure::Legacy);
            }
            if !callback(state, AuthorityAction::AuthorizeAtomic(&command)) {
                return Err(BatchError::new(
                    ToolErrorCode::ToolFailed,
                    "atomic command refused by authority",
                )
                .at(index)
                .into());
            }
            let clipboard = command.changes_clipboard();
            read_cache.clear();
            match edit.apply(state, command) {
                Ok(()) => (),
                Err(AtomicAuthorityError::Unsupported) => return Err(Failure::Legacy),
                Err(error) => {
                    return Err(
                        BatchError::new(ToolErrorCode::ToolFailed, error.to_string())
                            .at(index)
                            .into(),
                    )
                }
            }
            if clipboard {
                crate::mcp_serve::agent_context::validate_atomic_clipboard(state).map_err(
                    |error| BatchError::new(ToolErrorCode::ToolFailed, error.to_string()).at(index),
                )?;
            }
            count += 1;
        }
        let new_ids = edit.finish_identity_step(state).map_err(|error| {
            BatchError::new(ToolErrorCode::ToolFailed, error.to_string()).at(index)
        })?;
        let result = json!({"index":index,"name":step.name,"result":result,"newIds":new_ids,
            "selectedIds":state.selection.set.iter().map(|id| id.as_str()).collect::<Vec<_>>()});
        result_bytes = result_bytes.saturating_add(result.to_string().len());
        if result_bytes > MAX_RESULT_BYTES {
            return Err(BatchError::argument("step results exceed 4 MiB")
                .at(index)
                .into());
        }
        if let Some(binding) = step.bind {
            bindings.insert(binding, index);
        }
        results.push(result);
    }
    if count != 0 && arguments.contains_key("agentId") {
        crate::mcp_serve::agent_context::validate_atomic_ui(state)
            .map_err(|error| BatchError::new(ToolErrorCode::ToolFailed, error.to_string()))?;
    }
    Ok((json!({"atomic":true,"operationCount":results.len(),"commandCount":count,
        "documentChanged":state.document_revision()!=initial_revision,"newIdsOrder":"document-preorder","results":results}).to_string(), count))
}
fn structural(name: &str) -> bool {
    matches!(
        name,
        "insert_node"
            | "delete_node"
            | "move_node"
            | "copy_node"
            | "replace_node"
            | "duplicate_selected"
            | "delete_selected"
            | "cut_selected"
            | "group_selected"
            | "ungroup_selected"
            | "paste_clipboard"
    )
}
fn eligible(name: &str) -> bool {
    cacheable_document_read(name)
        || structural(name)
        || matches!(
            name,
            "update_node"
                | "set_selection"
                | "set_selection_set"
                | "clear_selection"
                | "toggle_node_selection"
                | "set_viewport"
                | "set_active_tool"
                | "set_active_page"
                | "copy_selected"
                | "cut_selected"
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
                | "set_effect_param"
                | "set_effect_color"
                | "set_variable_number"
                | "set_variable_string"
                | "set_variable_boolean"
                | "set_variable_color"
                | "set_variables"
                | "upsert_variables"
                | "set_themes"
                | "create_variable"
                | "delete_variable"
                | "rename_variable"
                | "set_active_axis_value"
                | "cycle_active_axis_value"
        )
}
