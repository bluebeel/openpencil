use std::collections::{BTreeMap, HashSet};

use jian_ops_schema::conversion::{ConversionEntry, ConversionKind};
use op_editor_core::{EditorState, PenNodeExt};

use super::{McpTool, ToolErrorCode, ToolOutcome};

pub struct ConversionStatus {
    entries: Vec<StatusEntry>,
}

struct StatusEntry {
    kind: String,
    key: String,
    source_path: Option<String>,
    source_hash: Option<String>,
    node_id: Option<String>,
    status: String,
}

impl McpTool for ConversionStatus {
    fn name(&self) -> &str {
        "conversion_status"
    }

    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        let filter = match args.get("kind").map(String::as_str) {
            None | Some("") => None,
            Some("token") => Some("token"),
            Some("component") => Some("component"),
            Some("screen") => Some("screen"),
            Some(other) => {
                return ToolOutcome::Err(
                    ToolErrorCode::InvalidArgument,
                    format!("kind must be token, component, or screen; got {other:?}"),
                );
            }
        };
        let entries = self
            .entries
            .iter()
            .filter(|entry| filter.is_none_or(|kind| entry.kind == kind));
        ToolOutcome::OkJson(status_json(entries))
    }
}

pub fn conversion_status_snapshot(state: &EditorState) -> ConversionStatus {
    let missing = missing_node_ids(&state.doc);
    let entries = state
        .doc
        .conversion
        .as_ref()
        .map(|spec| {
            spec.entries
                .iter()
                .map(|entry| status_entry(entry, &missing))
                .collect()
        })
        .unwrap_or_default();
    ConversionStatus { entries }
}

// Retain only ledger identities. One borrowed forest walk replaces a complete
// document search for every converted root; tokens require no tree traversal.
fn missing_node_ids(doc: &jian_ops_schema::PenDocument) -> HashSet<&str> {
    let mut missing: HashSet<_> = doc
        .conversion
        .iter()
        .flat_map(|spec| &spec.entries)
        .filter(|entry| entry.kind != ConversionKind::Token)
        .filter_map(|entry| entry.node_id.as_deref())
        .collect();
    for roots in doc
        .pages
        .iter()
        .flatten()
        .map(|page| page.children.as_slice())
        .chain(std::iter::once(doc.children.as_slice()))
    {
        if missing.is_empty() {
            break;
        }
        let mut pending = vec![roots.iter()];
        while let Some(siblings) = pending.last_mut() {
            if let Some(node) = siblings.next() {
                missing.remove(node.id_str());
                if missing.is_empty() {
                    break;
                }
                if let Some(children) = node.children() {
                    pending.push(children.iter());
                }
            } else {
                pending.pop();
            }
        }
    }
    missing
}
fn status_entry(entry: &ConversionEntry, missing: &HashSet<&str>) -> StatusEntry {
    let node_ok = match entry.kind {
        ConversionKind::Token => true,
        ConversionKind::Component | ConversionKind::Screen => entry
            .node_id
            .as_deref()
            .is_some_and(|node_id| !node_id.is_empty() && !missing.contains(node_id)),
    };
    StatusEntry {
        kind: kind_str(entry.kind).into(),
        key: entry.key.clone(),
        source_path: entry.source_path.clone(),
        source_hash: entry.source_hash.clone(),
        node_id: entry.node_id.clone(),
        status: if node_ok { "ok" } else { "orphaned" }.into(),
    }
}

fn kind_str(kind: ConversionKind) -> &'static str {
    match kind {
        ConversionKind::Token => "token",
        ConversionKind::Component => "component",
        ConversionKind::Screen => "screen",
    }
}

fn status_json<'a>(entries: impl Iterator<Item = &'a StatusEntry>) -> String {
    let values: Vec<serde_json::Value> = entries.map(status_entry_json).collect();
    serde_json::json!({
        "total": values.len(),
        "entries": values,
    })
    .to_string()
}

fn status_entry_json(entry: &StatusEntry) -> serde_json::Value {
    let mut value = serde_json::Map::new();
    value.insert("kind".into(), entry.kind.clone().into());
    value.insert("key".into(), entry.key.clone().into());
    if let Some(source_path) = &entry.source_path {
        value.insert("sourcePath".into(), source_path.clone().into());
    }
    if let Some(source_hash) = &entry.source_hash {
        value.insert("sourceHash".into(), source_hash.clone().into());
    }
    if let Some(node_id) = &entry.node_id {
        value.insert("nodeId".into(), node_id.clone().into());
    }
    value.insert("status".into(), entry.status.clone().into());
    serde_json::Value::Object(value)
}
