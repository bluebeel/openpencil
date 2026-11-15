//! TS-compatible `read_nodes` tool for codegen and inspection flows.

use std::collections::BTreeMap;

use jian_ops_schema::node::PenNode;
use op_editor_core::EditorState;
use serde_json::Value;

use super::{McpTool, ToolErrorCode, ToolOutcome};

type ToolCallError = (ToolErrorCode, String);

pub struct ReadNodes {
    pages: Vec<PageNodes>,
    active_page_id: String,
    variables_json: String,
    themes_json: String,
    prepared: Option<ToolOutcome>,
}

#[derive(Clone)]
pub(crate) struct PageNodes {
    pub(crate) id: String,
    pub(crate) roots: Vec<PenNode>,
}

impl McpTool for ReadNodes {
    fn name(&self) -> &str {
        "read_nodes"
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
        let page = match self.page_nodes(args) {
            Ok(page) => page,
            Err((code, msg)) => return ToolOutcome::Err(code, msg),
        };
        read_nodes_result(
            &page.roots,
            args,
            |ids| super::requested_nodes::find(&page.roots, ids),
            || {
                (
                    serde_json::from_str(&self.variables_json)
                        .unwrap_or_else(|_| serde_json::json!({})),
                    serde_json::from_str(&self.themes_json)
                        .unwrap_or_else(|_| serde_json::json!([])),
                )
            },
        )
    }
}

fn read_nodes_result<'a>(
    roots: &'a [PenNode],
    args: &BTreeMap<String, String>,
    find: impl FnOnce(&[String]) -> Vec<&'a PenNode>,
    metadata: impl FnOnce() -> (Value, Value),
) -> ToolOutcome {
    let depth = match parse_depth(args) {
        Ok(depth) => depth,
        Err((code, msg)) => return ToolOutcome::Err(code, msg),
    };
    let include_variables = match parse_include_variables(args) {
        Ok(v) => v,
        Err((code, msg)) => return ToolOutcome::Err(code, msg),
    };
    let node_ids = match parse_node_ids(args) {
        Ok(ids) => ids,
        Err((code, msg)) => return ToolOutcome::Err(code, msg),
    };

    // Match TS read-nodes EXACTLY: { nodes, variables?, themes? } with
    // NATIVE JSON values and NO Rust-only `count` key (TS ReadNodesResult
    // has none; cf. batch_get which also dropped `count`). The old path
    // stringified the nodes/variables/themes arrays into String values.
    let metadata = || include_variables.then(metadata);
    let encoded = match node_ids {
        Some(ids) if !ids.is_empty() => {
            node_snapshot_stream::encode(find(&ids), depth, |_| {}, metadata)
        }
        _ => node_snapshot_stream::encode(roots, depth, |_| {}, metadata),
    };
    match encoded {
        Ok(json) => ToolOutcome::OkJson(json),
        Err(error) => ToolOutcome::Err(
            ToolErrorCode::Internal,
            format!("serialize nodes failed: {error}"),
        ),
    }
}

/// Request-specific immutable result; call with the same arguments used here.
/// Reads borrowed canonical roots and retains only the requested reply, never
/// a clone of the page forests. The legacy constructor still supports any args.
pub fn read_nodes_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> ReadNodes {
    let outcome = match requested_page_roots(state, args) {
        Ok(roots) => read_nodes_result(
            roots,
            args,
            |ids| super::requested_nodes::find_for_state(state, roots, ids),
            || {
                (
                    state
                        .doc
                        .variables
                        .as_ref()
                        .map(|value| {
                            serde_json::to_value(value).unwrap_or_else(|_| serde_json::json!({}))
                        })
                        .unwrap_or_else(|| serde_json::json!({})),
                    state
                        .doc
                        .themes
                        .as_ref()
                        .map(|value| {
                            serde_json::to_value(value).unwrap_or_else(|_| serde_json::json!([]))
                        })
                        .unwrap_or_else(|| serde_json::json!([])),
                )
            },
        ),
        Err((code, message)) => ToolOutcome::Err(code, message),
    };
    ReadNodes {
        pages: Vec::new(),
        active_page_id: String::new(),
        variables_json: String::new(),
        themes_json: String::new(),
        prepared: Some(outcome),
    }
}

/// Same page aliases, implicit-page fallback and errors as the owned snapshots.
pub(crate) fn requested_page_roots<'a>(
    state: &'a EditorState,
    args: &BTreeMap<String, String>,
) -> Result<&'a [PenNode], ToolCallError> {
    let pages = state.doc.pages.as_deref().unwrap_or_default();
    let active = pages.get(
        state
            .ui
            .active_page_index
            .min(pages.len().saturating_sub(1)),
    );
    let default_id = active.map_or("0", |page| page.id.as_str());
    let target = arg_alias(args, &["pageId", "page_id", "page"]).unwrap_or(default_id);
    if pages.is_empty() && target == "0" {
        return Ok(&state.doc.children);
    }
    pages
        .iter()
        .find(|page| page.id == target)
        .map(|page| page.children.as_slice())
        .ok_or_else(|| {
            (
                ToolErrorCode::ToolFailed,
                format!("page not found: {target}"),
            )
        })
}

pub fn read_nodes_snapshot(state: &EditorState) -> ReadNodes {
    let (pages, active_page_id) = page_nodes_snapshots(state);
    // Match TS read-nodes EXACTLY (handleReadNodes): variables = doc.variables
    // ?? {} (absent → empty OBJECT); themes = doc.themes ?? [] (absent → empty
    // ARRAY, NOT an empty object). `unwrap_or_default()` would have collapsed an
    // absent themes map to `{}`, diverging from TS's `[]` for variable-only docs.
    let variables_json = match state.doc.variables.as_ref() {
        Some(v) => serde_json::to_string(v).unwrap_or_else(|_| "{}".into()),
        None => "{}".into(),
    };
    let themes_json = match state.doc.themes.as_ref() {
        Some(t) => serde_json::to_string(t).unwrap_or_else(|_| "[]".into()),
        None => "[]".into(),
    };
    ReadNodes {
        pages,
        active_page_id,
        variables_json,
        themes_json,
        prepared: None,
    }
}

impl ReadNodes {
    fn page_nodes(&self, args: &BTreeMap<String, String>) -> Result<&PageNodes, ToolCallError> {
        let target =
            arg_alias(args, &["pageId", "page_id", "page"]).unwrap_or(&self.active_page_id);
        self.pages
            .iter()
            .find(|page| page.id == target)
            .ok_or_else(|| {
                (
                    ToolErrorCode::ToolFailed,
                    format!("page not found: {target}"),
                )
            })
    }
}

pub(crate) fn page_nodes_snapshots(state: &EditorState) -> (Vec<PageNodes>, String) {
    match state.doc.pages.as_ref() {
        Some(pages) if !pages.is_empty() => {
            let active_idx = state
                .ui
                .active_page_index
                .min(pages.len().saturating_sub(1));
            let active_page_id = pages[active_idx].id.clone();
            let pages = pages
                .iter()
                .map(|page| PageNodes {
                    id: page.id.clone(),
                    roots: page.children.clone(),
                })
                .collect();
            (pages, active_page_id)
        }
        _ => (
            vec![PageNodes {
                id: "0".into(),
                roots: state.doc.children.clone(),
            }],
            "0".into(),
        ),
    }
}

fn parse_depth(args: &BTreeMap<String, String>) -> Result<i32, ToolCallError> {
    match args.get("depth") {
        None => Ok(-1),
        Some(raw) => raw.parse::<i32>().map_err(|_| {
            (
                ToolErrorCode::InvalidArgument,
                format!("depth must be an i32, got {raw:?}"),
            )
        }),
    }
}

fn parse_include_variables(args: &BTreeMap<String, String>) -> Result<bool, ToolCallError> {
    match arg_alias(args, &["includeVariables", "include_variables", "vars"]) {
        None => Ok(false),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(raw) => Err((
            ToolErrorCode::InvalidArgument,
            format!("includeVariables must be true or false, got {raw:?}"),
        )),
    }
}

fn parse_node_ids(args: &BTreeMap<String, String>) -> Result<Option<Vec<String>>, ToolCallError> {
    let Some(raw) = arg_alias(args, &["nodeIds", "node_ids", "ids"]) else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Some(Vec::new()));
    }
    if trimmed.starts_with('[') {
        return serde_json::from_str::<Vec<String>>(trimmed)
            .map(Some)
            .map_err(|e| {
                (
                    ToolErrorCode::InvalidArgument,
                    format!("nodeIds must be a JSON string array: {e}"),
                )
            });
    }
    Ok(Some(
        trimmed
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect(),
    ))
}

pub(crate) fn node_snapshot_value(node: &PenNode, depth: i32) -> Value {
    node_snapshot::value(node, depth)
}

#[path = "node_snapshot.rs"]
mod node_snapshot;

#[path = "node_snapshot_stream.rs"]
mod node_snapshot_stream;
pub(crate) use node_snapshot_stream::{
    encode as encode_node_snapshots, encode_with_fields as encode_node_fields,
};

fn arg_alias<'a>(args: &'a BTreeMap<String, String>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| args.get(*key).map(String::as_str))
}
