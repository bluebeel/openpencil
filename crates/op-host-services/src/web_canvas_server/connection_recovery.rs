//! Recovery is an authority operation, not an actor edit or alternate-file tool.

use super::connection::{admit_mutation, ConnCtx, MutationAdmission};
use super::Result;
use crate::mcp_serve::{self, tool_profile::McpAccessProfile};
use op_mcp::{ToolCall, ToolErrorCode, ToolResponse};

/// Runs before the primary fence and file router. A recovery call cannot name
/// a path, and the authority lock protects both its binding and replacement.
/// Static protocol discovery remains available without reading uncertain data.
pub(super) fn dispatch(
    ctx: &ConnCtx<'_>,
    line: &str,
    profile: McpAccessProfile,
) -> Option<Result<String>> {
    if ctx.authority.recovery.required() {
        if let Some(response) = mcp_serve::fenced_protocol_response(line, profile) {
            return Some(Ok(response));
        }
    }
    let call = op_mcp::parse_tool_call(line.trim())?;
    if call.tool != "reload_document" {
        return None;
    }
    Some(Ok(op_mcp::tool_response_to_json(&reload(
        ctx, &call, profile,
    ))))
}

fn reload(ctx: &ConnCtx<'_>, call: &ToolCall, profile: McpAccessProfile) -> ToolResponse {
    // Policy enforcement precedes argument validation and any document/path
    // access, exactly as it does for the ordinary registry dispatcher.
    if let Some(refusal) = profile.refuse_call(&call.tool, &call.arguments) {
        return error(call, ToolErrorCode::ToolFailed, refusal.message(&call.tool));
    }
    if let Err(reason) = op_mcp::reload_document::validate_arguments(&call.arguments) {
        return error(call, ToolErrorCode::InvalidArgument, reason.to_string());
    }
    if !ctx.mode.allows_local_file_routes() {
        return error(
            call,
            ToolErrorCode::ToolFailed,
            "reload-document-unavailable: recovery requires a local bound document".into(),
        );
    }
    let _write_pass = match admit_mutation(ctx, true) {
        MutationAdmission::Admitted(pass) => pass,
        MutationAdmission::ShuttingDown => {
            return error(
                call,
                ToolErrorCode::ToolFailed,
                "shutting-down: this daemon is stopping and cannot reload its document".into(),
            );
        }
        MutationAdmission::NotAWrite => unreachable!("reload is a document mutation"),
    };
    let mut state = ctx
        .state
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if state.current_path.is_none() {
        return error(
            call,
            ToolErrorCode::ToolFailed,
            "reload-document-unavailable: the daemon has no original bound file; no document was replaced and no request was replayed".into(),
        );
    }
    let was_fenced = state.recovery.required();
    let before_version = state.version;
    if let Err(reason) = state.reload_authoritative_file() {
        return error(
            call,
            ToolErrorCode::ToolFailed,
            format!(
                "document-recovery-required: {reason}; the authority remains fenced; no request was replayed"
            ),
        );
    }
    // The existing verified reload advances the recovery epoch. Old queued
    // groups must re-check that epoch and cannot apply to this replacement.
    // Never enter an actor scope here: it could restore pre-recovery UI state.
    ctx.authority.refresh_route(&state);
    ctx.state.clear_poison();
    if state.version != before_version {
        ctx.hub.broadcast(state.sse_tick());
    }
    ToolResponse::Ok {
        id: call.id.clone(),
        result: Default::default(),
        command: None,
        json: Some(
            serde_json::json!({
                "ok":true,
                "reloaded":was_fenced,
                "replay":false,
                "version":state.version,
                "documentGeneration":state.editor.document_generation(),
                "recoveryEpoch":state.recovery.epoch(),
                "refreshTools":was_fenced
            })
            .to_string(),
        ),
        image: None,
    }
}

fn error(call: &ToolCall, code: ToolErrorCode, message: String) -> ToolResponse {
    ToolResponse::Err {
        id: call.id.clone(),
        code,
        message,
    }
}
