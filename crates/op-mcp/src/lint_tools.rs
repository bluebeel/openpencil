use super::{McpTool, ToolErrorCode, ToolOutcome};
use op_design_lint::Issue;
use op_editor_core::EditorState;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
#[path = "lint_tools_cache.rs"]
mod cache;
#[path = "lint_tools_scan.rs"]
mod scan;
type Prepared = Result<String, String>;
pub struct LintDocument {
    data: Data,
}
enum Data {
    Full {
        issues: Vec<Issue>,
        subtrees: scan::Subtrees,
    },
    Call {
        filter: Option<String>,
        result: Arc<cache::ResultValue>,
    },
}
fn filter(args: &BTreeMap<String, String>) -> Option<&str> {
    args.get("nodeId")
        .map(|value| value.trim())
        .filter(|id| !id.is_empty())
}
fn render(issues: &[Issue], allowed: Option<&BTreeSet<&str>>) -> String {
    let issues = issues
        .iter()
        .filter(|issue| allowed.is_none_or(|ids| ids.contains(issue.node_id.as_str())))
        .collect::<Vec<_>>();
    serde_json::json!({"count":issues.len(),"issues":issues}).to_string()
}
fn outcome(result: &Prepared) -> ToolOutcome {
    match result {
        Ok(json) => ToolOutcome::OkJson(json.clone()),
        Err(error) => ToolOutcome::Err(ToolErrorCode::InvalidArgument, error.clone()),
    }
}
impl McpTool for LintDocument {
    fn name(&self) -> &str {
        "lint_document"
    }
    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        match &self.data {
            Data::Call {
                filter: prepared,
                result,
            } => {
                if filter(args) != prepared.as_deref() {
                    return ToolOutcome::Err(
                        ToolErrorCode::InvalidArgument,
                        "lint snapshot was prepared for a different nodeId".into(),
                    );
                }
                outcome(&result.result)
            }
            Data::Full { issues, subtrees } => {
                let allowed = match filter(args) {
                    None => None,
                    Some(id) => match subtrees.allowed(id) {
                        Some(ids) => Some(ids),
                        None => return outcome(&Err(format!("nodeId not found: {id}"))),
                    },
                };
                ToolOutcome::OkJson(render(issues, allowed.as_ref()))
            }
        }
    }
}
/// Reusable public snapshot: always inspect the actual mutable Doc afresh.
pub fn lint_document_snapshot(state: &EditorState) -> LintDocument {
    let mut subtrees = scan::Subtrees::default();
    let mut issues = Vec::new();
    for root in scan::roots(state) {
        subtrees.push(root);
        issues.extend(op_design_lint::detect_all(root, &state.doc));
    }
    LintDocument {
        data: Data::Full { issues, subtrees },
    }
}
/// Request-scoped constructor; only the normalized nodeId may be reused by call.
pub fn lint_document_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> LintDocument {
    snapshot_for_call(state, args, false)
}
/// Requires an explicit locked immutable host authority. A91 lexical handle is
/// checked too: arbitrary public callers never acquire cache trust by revision.
pub fn lint_document_snapshot_for_call_cached(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> LintDocument {
    snapshot_for_call(
        state,
        args,
        state.trusted_node_lookup(&state.doc.children).is_some(),
    )
}
fn snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
    trusted: bool,
) -> LintDocument {
    let filter = filter(args).map(str::to_owned);
    let result = cache::result(state, filter.as_deref(), trusted, || {
        scan::prepare(state, filter.as_deref())
    });
    LintDocument {
        data: Data::Call { filter, result },
    }
}
