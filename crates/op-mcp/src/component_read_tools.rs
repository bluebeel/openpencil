//! Component catalog read snapshots; moved from tools.rs before optimization.
use super::{count_subtree, kind_label};
use super::{McpTool, ToolErrorCode, ToolOutcome};
use op_editor_core::EditorState;
use std::collections::BTreeMap;
// --- list_components / get_component ---------------------------------

/// First-party `list_components` tool.
pub struct ListComponents {
    pub items: Vec<(String, String)>,
}

impl McpTool for ListComponents {
    fn name(&self) -> &str {
        "list_components"
    }
    fn call(&self, _args: &BTreeMap<String, String>) -> ToolOutcome {
        let encoded = catalog_records(
            self.items
                .iter()
                .map(|(name, id)| (name.as_str(), id.as_str())),
        );
        let mut out = BTreeMap::new();
        out.insert("count".into(), self.items.len().to_string());
        out.insert("components".into(), encoded);
        ToolOutcome::Ok(out)
    }
}

pub fn list_components_snapshot(state: &EditorState) -> ListComponents {
    ListComponents {
        items: state
            .components
            .components
            .iter()
            .map(|c| (c.name.clone(), c.id.as_str().to_string()))
            .collect(),
    }
}

/// First-party `get_component` tool.
pub struct GetComponent {
    pub snapshot: Vec<(String, String, String, usize)>,
}

impl McpTool for GetComponent {
    fn name(&self) -> &str {
        "get_component"
    }
    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        let Some(raw) = args.get("component_id") else {
            return ToolOutcome::Err(
                ToolErrorCode::MissingArgument,
                "component_id is required".into(),
            );
        };
        let component_id: &str = raw.as_str();
        let Some((_, name, kind, leaf_count)) = self
            .snapshot
            .iter()
            .find(|(id, _, _, _)| id == component_id)
        else {
            return ToolOutcome::Err(
                ToolErrorCode::ToolFailed,
                format!("component {component_id} not found"),
            );
        };
        let mut out = BTreeMap::new();
        out.insert("name".into(), name.clone());
        out.insert("kind".into(), kind.clone());
        out.insert("leaf_count".into(), leaf_count.to_string());
        ToolOutcome::Ok(out)
    }
}

pub fn get_component_snapshot(state: &EditorState) -> GetComponent {
    GetComponent {
        snapshot: state
            .components
            .components
            .iter()
            .filter_map(|c| {
                let root = state.components.resolved_root(&state.doc, &c.id)?;
                Some((
                    c.id.as_str().to_string(),
                    c.name.clone(),
                    kind_label(root).to_string(),
                    count_subtree(root),
                ))
            })
            .collect(),
    }
}

/// Stream escaped fields into one final record buffer. IDs retain the original
/// unescaped wire semantics; names use the same three escape characters.
pub(crate) fn catalog_records<'a>(items: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut out = String::new();
    for (name, id) in items {
        if !out.is_empty() {
            out.push(';');
        }
        append_record_field(&mut out, name);
        out.push('|');
        out.push_str(id);
    }
    out
}
pub(crate) fn append_record_field(out: &mut String, value: &str) {
    for c in value.chars() {
        if matches!(c, '\\' | ';' | '|') {
            out.push('\\');
        }
        out.push(c);
    }
}
fn requested_record(state: &EditorState, id: &str) -> Option<(String, String, String, usize)> {
    state
        .components
        .components
        .iter()
        .filter(|c| c.id.as_str() == id)
        .find_map(|c| {
            let root = state.components.resolved_root(&state.doc, &c.id)?;
            Some((
                c.id.as_str().into(),
                c.name.clone(),
                kind_label(root).into(),
                count_subtree(root),
            ))
        })
}
/// This constructor is bound to the supplied request. The public arbitrary-args
/// snapshot above retains its complete detached catalog.
pub fn get_component_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> GetComponent {
    GetComponent {
        snapshot: args
            .get("component_id")
            .and_then(|id| requested_record(state, id))
            .into_iter()
            .collect(),
    }
}
/// Requires the existing locked-authority canonical revision contract. Only a
/// unique document-backed prototype may reuse a counted subtree; custom owned
/// prototypes remain fresh, with identical first-record behavior.
pub fn get_component_snapshot_cached_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> GetComponent {
    let Some(id) = args.get("component_id").filter(|id| id.len() <= 4096) else {
        return get_component_snapshot_for_call(state, args);
    };
    let record = crate::document_summary::component_record_cached(state, id, || {
        let mut entries = state
            .components
            .components
            .iter()
            .filter(|c| c.id.as_str() == id);
        let entry = entries.next()?;
        if entries.next().is_some() || !state.components.is_document_backed(&entry.id) {
            return None;
        }
        let root = state.components.resolved_root(&state.doc, &entry.id)?;
        Some((
            entry.id.as_str().into(),
            entry.name.clone(),
            kind_label(root).into(),
            count_subtree(root),
        ))
    });
    match record {
        Some(record) => GetComponent {
            snapshot: vec![record],
        },
        None => get_component_snapshot_for_call(state, args),
    }
}
