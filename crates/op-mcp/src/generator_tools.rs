//! MCP tools for canvas generator nodes (see `op_editor_core::generator`):
//! `create_generator`, `update_generator_params`, `get_generator`,
//! `detach_generator`.
//!
//! Every write tool validates and runs the program against its snapshot
//! and hands the host exactly ONE command — the same command the property
//! panel applies — so an agent edit is one undo step and a failing program
//! leaves the document untouched. Programs use the `batch_design { script }`
//! node vocabulary: `I(parent, node)` returns a binding, `root` is the
//! generator, `params.<name>` are the inputs.

use std::collections::BTreeMap;

use jian_ops_schema::node::PenNode;
use op_editor_core::generator::{
    GeneratorError, GeneratorParam, GeneratorRunner, GeneratorSpec, GeneratorStarter,
    GENERATOR_STARTERS,
};
use op_editor_core::{EditorState, NodeId, PenNodeExt};
use serde_json::{json, Map, Value};

use crate::{McpTool, ToolErrorCode, ToolOutcome};

/// The runtime MCP tools use: the in-crate QuickJS runner when this build
/// has it, else whatever the host installed (none on the browser bundle).
fn runner() -> Option<GeneratorRunner> {
    #[cfg(feature = "script")]
    {
        Some(crate::generator_runtime::run_generator)
    }
    #[cfg(not(feature = "script"))]
    {
        op_editor_core::generator::installed_generator_runner()
    }
}

fn invalid(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome::Err(ToolErrorCode::InvalidArgument, message.into())
}

fn failed(error: GeneratorError) -> ToolOutcome {
    // Argument problems are the caller's to fix; a program that ran and
    // failed (or a host without a runtime) is a tool failure.
    let code = match error {
        GeneratorError::NodeNotFound { .. }
        | GeneratorError::NotAGenerator { .. }
        | GeneratorError::NotAFrame { .. }
        | GeneratorError::InvalidSpec(_)
        | GeneratorError::UnsupportedEngine(_)
        | GeneratorError::UnknownParam(_)
        | GeneratorError::ParamIndexOutOfRange { .. }
        | GeneratorError::InvalidParamValue { .. }
        | GeneratorError::ProgramTooLarge { .. } => ToolErrorCode::InvalidArgument,
        _ => ToolErrorCode::ToolFailed,
    };
    ToolOutcome::Err(code, error.to_string())
}

fn node_id_arg(args: &BTreeMap<String, String>) -> Result<NodeId, Box<ToolOutcome>> {
    args.get("node_id")
        .or_else(|| args.get("nodeId"))
        .and_then(|raw| NodeId::new_opt(raw.trim()))
        .ok_or_else(|| {
            Box::new(ToolOutcome::Err(
                ToolErrorCode::MissingArgument,
                "node_id is required".into(),
            ))
        })
}

fn json_arg(args: &BTreeMap<String, String>, key: &str) -> Result<Option<Value>, Box<ToolOutcome>> {
    args.get(key)
        .map(|raw| {
            serde_json::from_str::<Value>(raw)
                .map_err(|error| Box::new(invalid(format!("{key} must be JSON: {error}"))))
        })
        .transpose()
}

fn find_node<'a>(state: &'a EditorState, id: &NodeId) -> Option<&'a PenNode> {
    match state.doc.pages.as_ref() {
        Some(pages) if !pages.is_empty() => pages
            .iter()
            .find_map(|page| op_editor_core::walkers::find_node(&page.children, id)),
        _ => op_editor_core::walkers::find_node(&state.doc.children, id),
    }
}

fn child_count(node: Option<&PenNode>) -> usize {
    node.and_then(PenNode::children).map_or(0, Vec::len)
}

/// `create_generator` — insert a generator frame from a built-in starter
/// or from `program` + `params`.
pub struct CreateGenerator {
    state: Option<EditorState>,
    prepared: Option<ToolOutcome>,
}

pub fn create_generator_snapshot(state: &EditorState) -> CreateGenerator {
    CreateGenerator {
        state: Some(state.clone()),
        prepared: None,
    }
}

fn generator_frame(args: &BTreeMap<String, String>) -> Result<PenNode, Box<ToolOutcome>> {
    let mut frame = json_arg(args, "frame")?.unwrap_or_else(|| {
        json!({ "layout": "vertical", "gap": 12, "padding": 16,
                "width": "fit_content", "height": "fit_content" })
    });
    let Some(object) = frame.as_object_mut() else {
        return Err(Box::new(invalid(
            "frame must be a JSON object of frame properties",
        )));
    };
    object.remove("children");
    object.insert("type".into(), json!("frame"));
    object.insert("id".into(), json!("generator"));
    object.entry("name").or_insert_with(|| json!("Generator"));
    for axis in ["x", "y"] {
        if let Some(raw) = args.get(axis) {
            let value: f64 = raw
                .trim()
                .parse()
                .map_err(|_| Box::new(invalid(format!("{axis} must be a number"))))?;
            object.insert(axis.into(), json!(value));
        }
    }
    if let Some(name) = args.get("name").filter(|name| !name.trim().is_empty()) {
        object.insert("name".into(), json!(name.trim()));
    }
    serde_json::from_value(frame)
        .map_err(|error| Box::new(invalid(format!("invalid frame: {error}"))))
}

impl McpTool for CreateGenerator {
    fn name(&self) -> &str {
        "create_generator"
    }
    fn call_once(mut self: Box<Self>, args: &BTreeMap<String, String>) -> ToolOutcome {
        match self.prepared.take() {
            Some(outcome) => outcome,
            None => self.call(args),
        }
    }

    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        if let Some(outcome) = &self.prepared {
            return outcome.clone();
        }
        match self.state.as_ref() {
            Some(state) => create_generator_call(state, args),
            None => ToolOutcome::Err(ToolErrorCode::Internal, "missing generator snapshot".into()),
        }
    }
}

/// Request-specific capture without copying unrelated pages or editor history.
pub fn create_generator_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> CreateGenerator {
    CreateGenerator {
        state: None,
        prepared: Some(create_generator_call(state, args)),
    }
}

fn create_generator_call(state: &EditorState, args: &BTreeMap<String, String>) -> ToolOutcome {
    let starter = args.get("starter").map(|id| id.trim());
    let (mut frame, spec) = match (starter, args.get("program")) {
        (Some(id), _) => {
            let Some(starter) = GeneratorStarter::by_id(id) else {
                let ids: Vec<&str> = GENERATOR_STARTERS.iter().map(|s| s.id).collect();
                return invalid(format!("unknown starter {id}; known: {}", ids.join(", ")));
            };
            (starter.frame(0.0, 0.0), starter.spec())
        }
        (None, Some(program)) => {
            let params: Vec<GeneratorParam> = match json_arg(args, "params") {
                Ok(Some(value)) => match serde_json::from_value(value) {
                    Ok(params) => params,
                    Err(error) => {
                        return invalid(format!(
                            "params must be an array of {{name, kind, value, label?}}: {error}"
                        ))
                    }
                },
                Ok(None) => Vec::new(),
                Err(outcome) => return *outcome,
            };
            let frame = match generator_frame(args) {
                Ok(frame) => frame,
                Err(outcome) => return *outcome,
            };
            (frame, GeneratorSpec::new(program.clone(), params))
        }
        (None, None) => {
            return ToolOutcome::Err(
                ToolErrorCode::MissingArgument,
                "pass either starter or program".into(),
            )
        }
    };
    if starter.is_some() {
        // Starters keep their own frame styling; placement still applies.
        match generator_frame(args) {
            Ok(placed) => {
                let base = frame.base_mut();
                base.x = placed.base().x.or(base.x);
                base.y = placed.base().y.or(base.y);
                if args.contains_key("name") {
                    base.name = placed.base().name.clone();
                }
            }
            Err(outcome) => return *outcome,
        }
    }
    let parent = args
        .get("parent_id")
        .or_else(|| args.get("parentId"))
        .and_then(|raw| NodeId::new_opt(raw.trim()))
        .unwrap_or(NodeId::NONE);
    let page_id = args
        .get("page_id")
        .or_else(|| args.get("pageId"))
        .map(String::as_str);
    match state.generator_create_command(&parent, page_id, frame, spec, runner()) {
        Ok((command, id)) => {
            let children = match &command {
                op_editor_core::EditorCommand::InsertAuthoredSubtreePreservingRoots {
                    nodes,
                    ..
                } => child_count(nodes.first()),
                _ => 0,
            };
            let result = json!({ "nodeId": id.as_str(), "childCount": children });
            ToolOutcome::OkJsonWithCommand(result.to_string(), command)
        }
        Err(error) => failed(error),
    }
}

/// `update_generator_params` — set parameter values (and optionally a new
/// program) and regenerate the owned children.
pub struct UpdateGeneratorParams {
    state: Option<EditorState>,
    prepared: Option<ToolOutcome>,
}

pub fn update_generator_params_snapshot(state: &EditorState) -> UpdateGeneratorParams {
    UpdateGeneratorParams {
        state: Some(state.clone()),
        prepared: None,
    }
}

impl McpTool for UpdateGeneratorParams {
    fn name(&self) -> &str {
        "update_generator_params"
    }
    fn call_once(mut self: Box<Self>, args: &BTreeMap<String, String>) -> ToolOutcome {
        match self.prepared.take() {
            Some(outcome) => outcome,
            None => self.call(args),
        }
    }

    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        if let Some(outcome) = &self.prepared {
            return outcome.clone();
        }
        match self.state.as_ref() {
            Some(state) => update_generator_params_call(state, args, false),
            None => ToolOutcome::Err(ToolErrorCode::Internal, "missing generator snapshot".into()),
        }
    }
}

/// Request-specific capture without copying unrelated pages or editor history.
pub fn update_generator_params_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> UpdateGeneratorParams {
    UpdateGeneratorParams {
        state: None,
        prepared: Some(update_generator_params_call(state, args, true)),
    }
}

fn update_generator_params_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
    request_specific: bool,
) -> ToolOutcome {
    let id = match node_id_arg(args) {
        Ok(id) => id,
        Err(outcome) => return *outcome,
    };
    let values: Map<String, Value> = match json_arg(args, "params") {
        Ok(Some(Value::Object(values))) => values,
        Ok(Some(_)) => return invalid("params must be a JSON object of name → value"),
        Ok(None) => Map::new(),
        Err(outcome) => return *outcome,
    };
    let program = args.get("program");
    // The new program (if any) and the new values ride in ONE command.
    let edit = |spec: &mut GeneratorSpec| {
        if let Some(program) = program {
            spec.program = program.clone();
        }
        spec.apply_param_values(&values)
    };
    match state.generator_rewrite_command(&id, &edit, runner()) {
        Ok(Some(command)) => {
            let children = if request_specific {
                // Core has already generated and validated this replacement.
                // Its complete children are in the emitted patch; counting
                // them needs neither a cloned Doc nor speculative application.
                match &command {
                    op_editor_core::EditorCommand::PatchNodeData { patch_json, .. } => {
                        crate::parser::array_field_len(patch_json, "children").unwrap_or(0)
                    }
                    _ => 0,
                }
            } else {
                // Preserve the legacy arbitrary-argument snapshot contract.
                let mut after = state.clone_for_command_simulation();
                if after.apply(command.clone()) {
                    child_count(find_node(&after, &id))
                } else {
                    0
                }
            };
            let result = json!({ "nodeId": id.as_str(), "changed": true, "childCount": children });
            ToolOutcome::OkJsonWithCommand(result.to_string(), command)
        }
        Ok(None) => {
            let result = json!({
                "nodeId": id.as_str(),
                "changed": false,
                "childCount": child_count(find_node(state, &id)),
            });
            ToolOutcome::OkJson(result.to_string())
        }
        Err(error) => failed(error),
    }
}

/// `get_generator` — read a generator's program, params, and ownership
/// state.
pub struct GetGenerator {
    state: Option<EditorState>,
    prepared: Option<ToolOutcome>,
}

pub fn get_generator_snapshot(state: &EditorState) -> GetGenerator {
    GetGenerator {
        state: Some(state.clone()),
        prepared: None,
    }
}

impl McpTool for GetGenerator {
    fn name(&self) -> &str {
        "get_generator"
    }
    fn call_once(mut self: Box<Self>, args: &BTreeMap<String, String>) -> ToolOutcome {
        match self.prepared.take() {
            Some(outcome) => outcome,
            None => self.call(args),
        }
    }

    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        if let Some(outcome) = &self.prepared {
            return outcome.clone();
        }
        match self.state.as_ref() {
            Some(state) => get_generator_call(state, args),
            None => ToolOutcome::Err(ToolErrorCode::Internal, "missing generator snapshot".into()),
        }
    }
}

/// Request-specific capture without copying unrelated pages or editor history.
pub fn get_generator_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> GetGenerator {
    GetGenerator {
        state: None,
        prepared: Some(get_generator_call(state, args)),
    }
}

fn get_generator_call(state: &EditorState, args: &BTreeMap<String, String>) -> ToolOutcome {
    let id = match node_id_arg(args) {
        Ok(id) => id,
        Err(outcome) => return *outcome,
    };
    match state.generator_read_summary(&id) {
        Ok((spec, child_count, children_edited)) => {
            let result = json!({
                "nodeId": id.as_str(),
                "program": spec.program,
                "params": spec.params,
                "starter": spec.starter,
                "childCount": child_count,
                "childrenEdited": children_edited,
                "starters": GENERATOR_STARTERS.iter().map(|s| s.id).collect::<Vec<_>>(),
            });
            ToolOutcome::OkJson(result.to_string())
        }
        Err(error) => failed(error),
    }
}

/// `detach_generator` — keep the children as plain nodes, drop the program.
pub struct DetachGenerator {
    state: Option<EditorState>,
    prepared: Option<ToolOutcome>,
}

pub fn detach_generator_snapshot(state: &EditorState) -> DetachGenerator {
    DetachGenerator {
        state: Some(state.clone()),
        prepared: None,
    }
}

impl McpTool for DetachGenerator {
    fn name(&self) -> &str {
        "detach_generator"
    }
    fn call_once(mut self: Box<Self>, args: &BTreeMap<String, String>) -> ToolOutcome {
        match self.prepared.take() {
            Some(outcome) => outcome,
            None => self.call(args),
        }
    }

    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        if let Some(outcome) = &self.prepared {
            return outcome.clone();
        }
        match self.state.as_ref() {
            Some(state) => detach_generator_call(state, args),
            None => ToolOutcome::Err(ToolErrorCode::Internal, "missing generator snapshot".into()),
        }
    }
}

/// Request-specific capture without copying unrelated pages or editor history.
pub fn detach_generator_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> DetachGenerator {
    DetachGenerator {
        state: None,
        prepared: Some(detach_generator_call(state, args)),
    }
}

fn detach_generator_call(state: &EditorState, args: &BTreeMap<String, String>) -> ToolOutcome {
    let id = match node_id_arg(args) {
        Ok(id) => id,
        Err(outcome) => return *outcome,
    };
    match state.generator_detach_command(&id) {
        Ok(command) => {
            let result = json!({ "nodeId": id.as_str(), "detached": true });
            ToolOutcome::OkJsonWithCommand(result.to_string(), command)
        }
        Err(error) => failed(error),
    }
}

#[cfg(all(test, feature = "script"))]
#[path = "generator_tools_tests.rs"]
mod tests;
