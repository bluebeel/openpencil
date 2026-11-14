//! TS-compatible codegen MCP tools (`codegen_plan` / `codegen_submit_chunk`
//! / `codegen_assemble` / `codegen_clean`).
//!
//! TS refs:
//! - tool layer: `packages/pen-mcp/src/tools/codegen-{plan,submit,assemble,clean}.ts`
//!   + `packages/pen-mcp/src/routes/codegen-routes.ts`
//! - live routes: `apps/web/server/api/mcp/codegen/{plan.post.ts,
//!   submit.post.ts,assemble/[id].get.ts,plan/[id].delete.ts}`
//! - pipeline state: `apps/web/server/utils/codegen-plan-store.ts`,
//!   ported in [`super::codegen_plan_store`].
//!
//! In TS the standalone `pen-mcp` server forwards these tools over HTTP to
//! the running web app because the pipeline state lives in app memory
//! (codegen-plan.ts:14-18 errors with "requires a running App" when no app
//! is up). The Rust MCP server *is* a long-lived process in every transport
//! (live `/mcp` HTTP, `--mcp` stdio, `--mcp-http`), so the plan store runs
//! in-process and the tools work everywhere.
//!
//! ## Documented divergences from TS
//!
//! - **File-backed mode works**: TS standalone-without-app always errors;
//!   the Rust file-backed server hosts the store itself, so external CLIs
//!   spawning `--mcp <path>` get the full plan→submit→assemble workflow
//!   (the TS error was an artifact of its split-process architecture, not
//!   a wire contract).
//! - **Error text**: TS tool errors embed the raw h3 error body
//!   (`codegen_plan failed: {"statusCode":400,…}`); Rust keeps the
//!   `codegen_<tool> failed: ` prefix but carries the clean route
//!   `statusMessage` instead of the JSON wrapper.
//! - **Page default**: the TS plan route resolves nodes via
//!   `getActivePageChildren(doc, pageId ?? null)` (plan.post.ts:33), which
//!   falls back to the FIRST page — including for unknown pageIds
//!   (pen-core tree-utils.ts:33-44). Mirrored here, unlike the other Rust
//!   read tools which default to the editor's active page.

use std::collections::BTreeMap;

use op_editor_core::EditorState;
use serde_json::Value;

use super::codegen_plan_store::{
    assemble_plan, clean_plan, create_plan_for_roots, submit_chunk_result,
};
use super::read_nodes::{page_nodes_snapshots, PageNodes};
use super::{McpTool, ToolErrorCode, ToolOutcome};

pub struct CodegenPlan {
    pages: Vec<PageNodes>,
    prepared: Option<ToolOutcome>,
}

pub struct CodegenSubmitChunk;
pub struct CodegenAssemble;
pub struct CodegenClean;

/// `JSON.stringify(value, null, 2)` — the TS tool layer pretty-prints
/// every codegen result before it rides the `content[].text` envelope
/// (codegen-plan.ts:33 etc).
fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

impl McpTool for CodegenPlan {
    fn name(&self) -> &str {
        "codegen_plan"
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
        // This tool deliberately defaults/falls back to the first page,
        // rather than the editor's active page.
        let page = args
            .get("pageId")
            .and_then(|id| self.pages.iter().find(|page| page.id == *id))
            .or_else(|| self.pages.first());
        codegen_plan_result(page.map_or(&[], |page| page.roots.as_slice()), args)
    }
}

fn codegen_plan_result(
    roots: &[jian_ops_schema::node::PenNode],
    args: &BTreeMap<String, String>,
) -> ToolOutcome {
    let Some(raw_plan) = args.get("plan") else {
        // TS: no plan in the forwarded body → plan.post.ts 400
        // "Missing plan in request body".
        return ToolOutcome::Err(
            ToolErrorCode::ToolFailed,
            "codegen_plan failed: Missing plan in request body".into(),
        );
    };
    let plan: Value = match serde_json::from_str(raw_plan) {
        Ok(value @ Value::Object(_)) => value,
        Ok(_) | Err(_) => {
            return ToolOutcome::Err(
                ToolErrorCode::InvalidArgument,
                "plan must be a JSON object".into(),
            );
        }
    };
    match create_plan_for_roots(&plan, roots) {
        Ok(result) => ToolOutcome::OkJson(pretty(&result)),
        Err(message) => ToolOutcome::Err(
            ToolErrorCode::ToolFailed,
            format!("codegen_plan failed: {message}"),
        ),
    }
}

impl McpTool for CodegenSubmitChunk {
    fn name(&self) -> &str {
        "codegen_submit_chunk"
    }

    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        let (Some(plan_id), Some(raw_result)) = (args.get("planId"), args.get("result")) else {
            // TS submit.post.ts 400 "Missing planId or result".
            return ToolOutcome::Err(
                ToolErrorCode::ToolFailed,
                "codegen_submit_chunk failed: Missing planId or result".into(),
            );
        };
        let result: Value = match serde_json::from_str(raw_result) {
            Ok(value @ Value::Object(_)) => value,
            Ok(_) | Err(_) => {
                return ToolOutcome::Err(
                    ToolErrorCode::InvalidArgument,
                    "result must be a JSON object".into(),
                );
            }
        };
        // TS submitChunkResult treats any non-failed/skipped override as
        // absent (`statusOverride === 'failed' || === 'skipped'`); the MCP
        // schema enum makes other values unreachable from TS clients.
        let status_override = args
            .get("status")
            .map(String::as_str)
            .filter(|s| matches!(*s, "failed" | "skipped"));
        match submit_chunk_result(plan_id, &result, status_override) {
            Ok(out) => ToolOutcome::OkJson(pretty(&out)),
            Err(message) => ToolOutcome::Err(
                ToolErrorCode::ToolFailed,
                format!("codegen_submit_chunk failed: {message}"),
            ),
        }
    }
}

impl McpTool for CodegenAssemble {
    fn name(&self) -> &str {
        "codegen_assemble"
    }

    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        let Some(plan_id) = args.get("planId") else {
            // TS assemble/[id].get.ts 400 "Missing plan ID".
            return ToolOutcome::Err(
                ToolErrorCode::ToolFailed,
                "codegen_assemble failed: Missing plan ID".into(),
            );
        };
        // TS route: `(query.framework as Framework) || 'react'` — no enum
        // validation at the route layer; the store ignores it anyway.
        let framework = args.get("framework").map(String::as_str).unwrap_or("react");
        match assemble_plan(plan_id, framework) {
            Ok(out) => ToolOutcome::OkJson(pretty(&out)),
            Err(message) => ToolOutcome::Err(
                ToolErrorCode::ToolFailed,
                format!("codegen_assemble failed: {message}"),
            ),
        }
    }
}

impl McpTool for CodegenClean {
    fn name(&self) -> &str {
        "codegen_clean"
    }

    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        let Some(plan_id) = args.get("planId") else {
            // TS plan/[id].delete.ts 400 "Missing plan ID".
            return ToolOutcome::Err(
                ToolErrorCode::ToolFailed,
                "codegen_clean failed: Missing plan ID".into(),
            );
        };
        ToolOutcome::OkJson(pretty(&clean_plan(plan_id)))
    }
}

pub fn codegen_plan_snapshot(state: &EditorState) -> CodegenPlan {
    let (pages, _active_page_id) = page_nodes_snapshots(state);
    CodegenPlan {
        pages,
        prepared: None,
    }
}

/// Resolve only the requested page and retain only this plan result. The
/// plan store serializes named chunk roots, not every node on every page.
pub fn codegen_plan_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> CodegenPlan {
    let pages = state.doc.pages.as_deref().unwrap_or_default();
    let roots = if pages.is_empty() {
        state.doc.children.as_slice()
    } else {
        args.get("pageId")
            .and_then(|id| pages.iter().find(|page| page.id == *id))
            .unwrap_or(&pages[0])
            .children
            .as_slice()
    };
    CodegenPlan {
        pages: Vec::new(),
        prepared: Some(codegen_plan_result(roots, args)),
    }
}

pub fn codegen_submit_chunk_snapshot() -> CodegenSubmitChunk {
    CodegenSubmitChunk
}

pub fn codegen_assemble_snapshot() -> CodegenAssemble {
    CodegenAssemble
}

pub fn codegen_clean_snapshot() -> CodegenClean {
    CodegenClean
}
