//! Request-scoped, command-backed authoring transactions. No live mutation
//! occurs during validation; one host command owns history and persistence.

use std::{collections::BTreeMap, sync::Mutex};

use op_editor_core::{AtomicCommandSimulation, EditorCommand, EditorState};
use op_mcp::{McpTool, ToolErrorCode, ToolOutcome, ToolResponse};
use serde_json::{json, Value};

use super::tool_profile::{self, McpAccessProfile, ToolAccess, ToolSurface};

#[path = "atomic_tools_ids.rs"]
mod ids;
#[path = "atomic_tools_input.rs"]
mod input;

const MAX_RESULT_BYTES: usize = 4 * 1024 * 1024;
const MAX_COMMANDS: usize = 1024;

pub(super) struct Unavailable;
impl McpTool for Unavailable {
    fn name(&self) -> &str {
        "batch_tools"
    }
    fn call(&self, _: &BTreeMap<String, String>) -> ToolOutcome {
        ToolOutcome::Err(ToolErrorCode::ToolFailed,
            "batch-tools-unavailable: this transport has no exclusively locked, durable, sequential-ID file authority; use file-backed stdio/HTTP or a bound Managed daemon".into())
    }
}

struct Prepared {
    arguments: BTreeMap<String, String>,
    outcome: Mutex<Option<ToolOutcome>>,
}

impl McpTool for Prepared {
    fn name(&self) -> &str {
        "batch_tools"
    }
    fn call(&self, arguments: &BTreeMap<String, String>) -> ToolOutcome {
        if arguments != &self.arguments {
            return ToolOutcome::Err(
                ToolErrorCode::InvalidArgument,
                "batch_tools request-scoped snapshot cannot run different arguments".into(),
            );
        }
        match self.outcome.lock() {
            Ok(mut outcome) => outcome.take().unwrap_or_else(|| {
                ToolOutcome::Err(
                    ToolErrorCode::ToolFailed,
                    "prepared batch_tools call was already consumed".into(),
                )
            }),
            Err(_) => ToolOutcome::Err(
                ToolErrorCode::Internal,
                "prepared batch_tools call is unavailable".into(),
            ),
        }
    }
}

pub(super) fn prepare(
    state: &EditorState,
    arguments: &BTreeMap<String, String>,
    profile: McpAccessProfile,
) -> Box<dyn McpTool> {
    let outcome = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        prepare_inner(state, arguments, profile)
    })) {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => ToolOutcome::Err(error.code, error.to_string()),
        Err(_) => ToolOutcome::Err(
            ToolErrorCode::ToolFailed,
            "batch-tools-refused: isolated validation panicked; no operations were applied".into(),
        ),
    };
    Box::new(Prepared {
        arguments: arguments.clone(),
        outcome: Mutex::new(Some(outcome)),
    })
}

#[derive(Debug)]
struct BatchError {
    code: ToolErrorCode,
    step: Option<usize>,
    message: String,
}

impl BatchError {
    fn new(code: ToolErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            step: None,
            message: message.into(),
        }
    }
    fn argument(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::InvalidArgument, message)
    }
    fn at(mut self, step: usize) -> Self {
        self.step = Some(step);
        self
    }
}
impl std::fmt::Display for BatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("batch-tools-refused")?;
        if let Some(step) = self.step {
            write!(f, " step={step}")?;
        }
        write!(f, ": {}; no operations were applied", self.message)
    }
}
impl std::error::Error for BatchError {}
type Result<T> = std::result::Result<T, BatchError>;

fn prepare_inner(
    state: &EditorState,
    arguments: &BTreeMap<String, String>,
    profile: McpAccessProfile,
) -> Result<ToolOutcome> {
    let steps = input::parse(arguments)?;
    // This pass runs before any registry constructor. An external-effect tool
    // can never run its validation as an accidental part of a failed plan.
    for (index, step) in steps.iter().enumerate() {
        allowed_name(&step.name, profile).map_err(|e| e.at(index))?;
    }
    let mut simulation = AtomicCommandSimulation::new(state);
    let initial_revision = state.document_revision();
    let mut identifiers = ids::Tracker::new(&state.doc);
    let mut commands = Vec::new();
    let mut results = Vec::new();
    let mut bindings = BTreeMap::new();
    let mut result_bytes = 0usize;
    let mut argument_bytes = 0usize;
    for (index, step) in steps.into_iter().enumerate() {
        let value = input::resolve(step.arguments, &results, &bindings).map_err(|e| e.at(index))?;
        let call =
            input::call(index, &step.name, value, &mut argument_bytes).map_err(|e| e.at(index))?;
        if let Some(refusal) = profile.refuse_call(&call.tool, &call.arguments) {
            return Err(
                BatchError::new(ToolErrorCode::ToolFailed, refusal.message(&call.tool)).at(index),
            );
        }
        input::one_authority(&call.arguments).map_err(|e| e.at(index))?;
        // Shipped templates are immutable inputs. A user-saved template reaches
        // process-global filesystem state even on an unrestricted local host.
        if call.tool == "use_scene_template"
            && call
                .arguments
                .get("templateId")
                .is_some_and(|name| name.trim().starts_with("user:"))
        {
            return Err(BatchError::argument(
                "user-saved templates are not atomic in-memory inputs",
            )
            .at(index));
        }
        let registry = super::registry::rebuild_registry(
            &simulation,
            Some(&call.tool),
            profile,
            Some(&call.arguments),
        );
        let (result, command) = outcome(registry.dispatch(call)).map_err(|e| e.at(index))?;
        let members = flatten(command).map_err(|e| e.at(index))?;
        if commands.len().saturating_add(members.len()) > MAX_COMMANDS {
            return Err(BatchError::argument("batch exceeds 1024 commands").at(index));
        }
        let mut changed_document = false;
        for command in members {
            if !command.supports_atomic_batch() {
                return Err(BatchError::argument(
                    "subtool returned an unsupported atomic command (undo/redo are not supported)",
                )
                .at(index));
            }
            changed_document |= command.may_change_document();
            let changes_clipboard = command.changes_clipboard();
            let accepted = simulation
                .apply(command.clone())
                .map_err(|e| BatchError::new(ToolErrorCode::ToolFailed, e.to_string()).at(index))?;
            if !accepted {
                return Err(BatchError::new(
                    ToolErrorCode::ToolFailed,
                    "subtool command rejected during isolated validation",
                )
                .at(index));
            }
            if changes_clipboard {
                super::agent_context::validate_atomic_clipboard(&simulation).map_err(|e| {
                    BatchError::new(ToolErrorCode::ToolFailed, e.to_string()).at(index)
                })?;
            }
            commands.push(command);
        }
        let new_ids = if changed_document {
            identifiers
                .created(&simulation.doc)
                .map_err(|e| e.at(index))?
        } else {
            Vec::new()
        };
        let result = json!({"index":index,"name":step.name,"result":result,
            "newIds":new_ids,"selectedIds":simulation.selection.set.iter()
                .map(|id| id.as_str()).collect::<Vec<_>>()});
        result_bytes = result_bytes.saturating_add(result.to_string().len());
        if result_bytes > MAX_RESULT_BYTES {
            return Err(BatchError::argument("step results exceed 4 MiB").at(index));
        }
        if let Some(binding) = step.bind {
            bindings.insert(binding, index);
        }
        results.push(result);
    }
    if arguments.contains_key("agentId") {
        super::agent_context::validate_atomic_ui(&simulation)
            .map_err(|e| BatchError::new(ToolErrorCode::ToolFailed, e.to_string()))?;
    }
    let result = json!({"atomic":true,"operationCount":results.len(),
        "commandCount":commands.len(),"documentChanged":simulation.document_revision()!=initial_revision,
        "newIdsOrder":"document-preorder","results":results});
    Ok(ToolOutcome::OkJsonWithCommand(
        result.to_string(),
        EditorCommand::Batch { commands },
    ))
}

fn allowed_name(name: &str, profile: McpAccessProfile) -> Result<()> {
    if let Some(refusal) = profile.refuse(name) {
        return Err(BatchError::new(
            ToolErrorCode::ToolFailed,
            refusal.message(name),
        ));
    }
    if name == "batch_tools" || super::write_repairs::generated_tool(name) {
        return Err(BatchError::argument(
            "nested batches and generated-design workflows use batch_design separately",
        ));
    }
    if let Some(classification) = tool_profile::profile_for(name) {
        if classification.surface != ToolSurface::InMemory {
            return Err(BatchError::argument("filesystem, network, process and host-side effects cannot participate in an atomic batch"));
        }
        if classification.access != ToolAccess::Write {
            return Err(BatchError::argument(
                "batch_tools accepts command-backed authoring tools, not read/export calls",
            ));
        }
    }
    Ok(())
}

fn outcome(response: ToolResponse) -> Result<(Value, EditorCommand)> {
    match response {
        ToolResponse::Err { code, message, .. } => Err(BatchError::new(code, message)),
        ToolResponse::Ok {
            command: Some(command),
            image: None,
            result,
            json,
            ..
        } => {
            let value = match json {
                Some(json) => {
                    if json.len() > MAX_RESULT_BYTES {
                        return Err(BatchError::argument("subtool result exceeds 4 MiB"));
                    }
                    serde_json::from_str(&json).map_err(|e| {
                        BatchError::new(
                            ToolErrorCode::Internal,
                            format!("subtool returned invalid JSON: {e}"),
                        )
                    })?
                }
                None => serde_json::to_value(result)
                    .map_err(|e| BatchError::new(ToolErrorCode::Internal, e.to_string()))?,
            };
            Ok((value, command))
        }
        ToolResponse::Ok { .. } => Err(BatchError::argument(
            "subtool must return a command and cannot return rendered image content",
        )),
    }
}

fn flatten(command: EditorCommand) -> Result<Vec<EditorCommand>> {
    let mut pending = vec![command];
    let mut commands = Vec::new();
    while let Some(command) = pending.pop() {
        match command {
            EditorCommand::Batch { commands: nested } => pending.extend(nested.into_iter().rev()),
            command => commands.push(command),
        }
        if commands.len().saturating_add(pending.len()) > MAX_COMMANDS {
            return Err(BatchError::argument(
                "expanded subtool exceeds 1024 commands",
            ));
        }
    }
    if commands.is_empty() {
        return Err(BatchError::argument(
            "subtool returned an empty command batch",
        ));
    }
    Ok(commands)
}

/// Admission reserves the existing clipboard bound before execution; names are
/// literal, so refs cannot conceal a later clipboard writer from this pass.
pub(super) fn requests_clipboard_reservation(line: &str) -> bool {
    op_mcp::parse_tool_call(line)
        .filter(|call| call.tool == "batch_tools")
        .and_then(|call| input::parse(&call.arguments).ok())
        .is_some_and(|steps| {
            steps
                .iter()
                .any(|step| matches!(step.name.as_str(), "copy_selected" | "cut_selected"))
        })
}
