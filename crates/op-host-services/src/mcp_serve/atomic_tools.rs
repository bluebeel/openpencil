//! Request-scoped, command-backed authoring transactions. No live mutation
//! occurs during validation; one host command owns history and persistence.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Mutex,
};

use op_editor_core::{AtomicCommandSimulation, EditorCommand, EditorState};
use op_mcp::{McpTool, ToolErrorCode, ToolOutcome, ToolResponse};
use serde_json::{json, Value};

use super::tool_profile::{self, McpAccessProfile, ToolAccess, ToolSurface};

#[path = "atomic_tools_ids.rs"]
mod ids;
#[path = "atomic_tools_input.rs"]
mod input;
#[path = "atomic_tools_sparse.rs"]
mod sparse;

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
    allow_writes: bool,
) -> Box<dyn McpTool> {
    let outcome = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        prepare_inner(state, arguments, profile, allow_writes)
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
    allow_writes: bool,
) -> Result<ToolOutcome> {
    let steps = input::parse(arguments)?;
    // This pass runs before any registry constructor. An external-effect tool
    // can never run its validation as an accidental part of a failed plan.
    for (index, step) in steps.iter().enumerate() {
        allowed_name(&step.name, profile).map_err(|e| e.at(index))?;
    }
    let reads_only = steps.iter().all(|step| {
        tool_profile::profile_for(&step.name)
            .is_some_and(|classification| classification.access == ToolAccess::Read)
    });
    if !allow_writes && !reads_only {
        return Err(BatchError::argument(
            "this authority accepts only read-only batches",
        ));
    }
    // Pure stateless setters can validate complete target headers without
    // constructing a partial document or cloning/seeding the whole source.
    if !reads_only {
        if let Some(outcome) = sparse::prepare(state, &steps, arguments, profile)? {
            return Ok(outcome);
        }
    }
    // The caller already holds one immutable document authority. Read batches
    // borrow it directly; cloning a huge document and seeding an ID allocator
    // for these calls adds no isolation or consistency.
    let mut simulation = (!reads_only).then(|| AtomicCommandSimulation::new(state));
    let initial_revision = state.document_revision();
    let mut identifiers: Option<ids::Tracker> = None;
    let mut commands = Vec::new();
    let mut results = Vec::new();
    let mut bindings = BTreeMap::new();
    // Repeated document reads share an answer until the next command. Keep
    // argument identity exact so rich references and requested depths differ.
    let mut read_cache: HashMap<(String, BTreeMap<String, String>), Value> = HashMap::new();
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
        let read_step = tool_profile::profile_for(&call.tool)
            .is_some_and(|classification| classification.access == ToolAccess::Read);
        if !allow_writes && !read_step {
            return Err(
                BatchError::argument("this authority accepts only read-only batches").at(index),
            );
        }
        let cache_key = (read_step && cacheable_document_read(&call.tool))
            .then(|| (call.tool.clone(), call.arguments.clone()));
        let (result, command) = match cache_key.as_ref().and_then(|key| read_cache.get(key)) {
            Some(result) => (result.clone(), None),
            None => {
                let registry = super::registry::rebuild_registry(
                    simulation.as_deref().unwrap_or(state),
                    Some(&call.tool),
                    profile,
                    Some(&call.arguments),
                );
                let outcome =
                    outcome(registry.dispatch(call), read_step).map_err(|e| e.at(index))?;
                if let Some(key) = cache_key {
                    read_cache.insert(key, outcome.0.clone());
                }
                outcome
            }
        };
        let members = match command {
            Some(command) => flatten(command).map_err(|e| e.at(index))?,
            None => Vec::new(),
        };
        if commands.len().saturating_add(members.len()) > MAX_COMMANDS {
            return Err(BatchError::argument("batch exceeds 1024 commands").at(index));
        }
        let mut changed_identities = false;
        for command in members {
            read_cache.clear();
            let simulation = simulation.as_mut().ok_or_else(|| {
                BatchError::argument("a read-only batch cannot return an authoring command")
                    .at(index)
            })?;
            if !command.supports_atomic_batch() {
                return Err(BatchError::argument(
                    "subtool returned an unsupported atomic command (undo/redo are not supported)",
                )
                .at(index));
            }
            if command.may_change_document_identities() {
                identifiers.get_or_insert_with(|| ids::Tracker::new(&simulation.doc));
                changed_identities = true;
            }
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
        let current = simulation.as_deref().unwrap_or(state);
        let new_ids = if changed_identities {
            identifiers
                .as_mut()
                .expect("identity tracker initialized before structural command")
                .created(&current.doc)
                .map_err(|e| e.at(index))?
        } else {
            Vec::new()
        };
        let result = json!({"index":index,"name":step.name,"result":result,
            "newIds":new_ids,"selectedIds":current.selection.set.iter()
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
    let current = simulation.as_deref().unwrap_or(state);
    if !commands.is_empty() && arguments.contains_key("agentId") {
        super::agent_context::validate_atomic_ui(current)
            .map_err(|e| BatchError::new(ToolErrorCode::ToolFailed, e.to_string()))?;
    }
    let result = json!({"atomic":true,"operationCount":results.len(),
        "commandCount":commands.len(),"documentChanged":current.document_revision()!=initial_revision,
        "newIdsOrder":"document-preorder","results":results});
    if commands.is_empty() {
        Ok(ToolOutcome::OkJson(result.to_string()))
    } else {
        Ok(ToolOutcome::OkJsonWithCommand(
            result.to_string(),
            EditorCommand::Batch { commands },
        ))
    }
}

// Runtime/host-status reads are intentionally uncached: they can advance
// independently of the immutable document held by this batch.
fn cacheable_document_read(name: &str) -> bool {
    matches!(
        name,
        "get_document_info"
            | "get_selection"
            | "get_node"
            | "get_node_children"
            | "get_node_parent"
            | "list_pages"
            | "list_variables"
            | "get_variables"
            | "get_active_theme"
            | "list_components"
            | "get_component"
            | "read_nodes"
            | "snapshot_layout"
            | "get_canvas_bounds"
            | "find_node_by_name"
            | "count_nodes"
            | "list_node_kinds"
            | "get_history_depth"
            | "get_viewport"
            | "get_selection_set"
            | "query_canvas"
    )
}

fn allowed_name(name: &str, profile: McpAccessProfile) -> Result<()> {
    if let Some(refusal) = profile.refuse(name) {
        return Err(BatchError::new(
            ToolErrorCode::ToolFailed,
            refusal.message(name),
        ));
    }
    if requires_host_work(name) {
        return Err(BatchError::argument(
            "render/export and mixed host-resource tools cannot run inside an in-memory batch",
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
    }
    Ok(())
}

/// Classification only: no registry is constructed and no operation runs.
/// Used for read-scope admission and safe read-only use on snapshot transports.
pub(super) fn read_only(arguments: &BTreeMap<String, String>) -> bool {
    input::parse(arguments).is_ok_and(|steps| {
        steps.iter().all(|step| {
            !requires_host_work(&step.name)
                && tool_profile::profile_for(&step.name).is_some_and(|classification| {
                    classification.access == ToolAccess::Read
                        && classification.surface == ToolSurface::InMemory
                })
        })
    })
}

fn requires_host_work(name: &str) -> bool {
    matches!(
        name,
        "get_screenshot"
            | "debug_screenshot"
            | "export_item"
            | "export_nodes"
            | "codegen_export"
            | "list_scene_templates"
            | "use_scene_template"
    )
}

fn outcome(response: ToolResponse, read_step: bool) -> Result<(Value, Option<EditorCommand>)> {
    match response {
        ToolResponse::Err { code, message, .. } => Err(BatchError::new(code, message)),
        ToolResponse::Ok {
            command,
            image: None,
            result,
            json,
            ..
        } => {
            if read_step != command.is_none() {
                return Err(BatchError::argument(
                    "read tools cannot return mutations; authoring tools must return a command",
                ));
            }
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
            "rendered image content is not supported inside an in-memory batch",
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
