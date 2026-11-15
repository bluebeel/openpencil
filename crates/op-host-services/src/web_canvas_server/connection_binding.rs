//! Acquire ownership before any document mutex wait. Denied routes retain
//! their existing validation/error order; no caller supplies a claim identity.
use super::{
    connection::ConnCtx, primary_files, primary_scope, Result, WebCanvasError,
    WebCredentialPersistence, WebReply,
};
use crate::mcp_serve::{self, tool_profile::McpAccessProfile, HttpRequest};
use std::{path::PathBuf, sync::Arc};
fn failed(error: impl std::fmt::Display) -> WebCanvasError {
    WebCanvasError::Config(format!("file authority: {error}"))
}
pub(super) fn refusal(error: impl std::fmt::Display) -> WebReply {
    WebReply{status:"503 Service Unavailable",body:serde_json::json!({"ok":false,"error":"file-authority-unavailable","message":error.to_string(),"replay":false}).to_string()}
}
pub(super) fn rest(ctx: &ConnCtx<'_>, req: &HttpRequest) -> Result<Option<primary_scope::Scope>> {
    if !ctx.mode.allows_local_file_routes() {
        return Ok(None);
    }
    let owner = Arc::clone(&ctx.authority.primary_files);
    if req.method == "POST" {
        match req.path.as_str() {
            "/api/file/open-recent" => {
                let path = serde_json::from_str::<serde_json::Value>(&req.body)
                    .ok()
                    .and_then(|value| {
                        value["path"]
                            .as_str()
                            .filter(|path| !path.trim().is_empty())
                            .map(str::to_owned)
                    });
                let Some(path) = path else { return Ok(None) };
                // Brief validation snapshot only. Drop it BEFORE claiming or
                // waiting on a resident/session. The real route revalidates.
                let admitted = {
                    let state = ctx.state.lock().unwrap_or_else(|p| p.into_inner());
                    !state.recovery.required()
                        && state
                            .editor
                            .editor_ui
                            .recent_files
                            .iter()
                            .any(|recent| recent.path == path)
                        && state
                            .collab_policy()
                            .check(
                                op_editor_core::CollabGateAction::ReplaceDocument,
                                op_editor_core::CollabEditSource::ExternalSync,
                            )
                            .is_ok()
                };
                if !admitted {
                    return Ok(None);
                }
                return primary_scope::Scope::bind(owner, Some(std::path::Path::new(&path)))
                    .map(Some)
                    .map_err(failed);
            }
            "/api/file/unbind" => {
                let fenced = ctx.authority.recovery.required();
                if fenced {
                    return Ok(None);
                }
                return primary_scope::Scope::bind(owner, None)
                    .map(Some)
                    .map_err(failed);
            }
            "/api/file/reload" | "/api/mcp/sync-reset" => {
                let path = {
                    let state = ctx.state.lock().unwrap_or_else(|p| p.into_inner());
                    if req.path == "/api/file/reload" && !state.recovery.required() {
                        return Ok(None);
                    }
                    if req.path == "/api/mcp/sync-reset"
                        && (state.recovery.required() || state.reset_consumed)
                    {
                        return Ok(None);
                    }
                    state.current_path.clone()
                };
                return primary_scope::Scope::bind(owner, path.as_deref())
                    .map(Some)
                    .map_err(failed);
            }
            _ => {}
        }
    }
    primary_scope::Scope::read(owner, ctx.authority.recovery.clone())
        .map(Some)
        .map_err(failed)
}
pub(super) fn mcp(
    ctx: &ConnCtx<'_>,
    line: &str,
    profile: McpAccessProfile,
) -> Result<Option<primary_scope::Scope>> {
    if ctx.mode.is_online() || mcp_serve::is_protocol_control(line) {
        return Ok(None);
    }
    let owner = Arc::clone(&ctx.authority.primary_files);
    if line.contains("reload_document") {
        if let Some(call) = op_mcp::parse_tool_call(line) {
            if call.tool == "reload_document" {
                // Recovery dispatcher owns exact policy/argument errors. A
                // refused call must not claim/load a path just to fail later.
                if profile.refuse_call(&call.tool, &call.arguments).is_some()
                    || op_mcp::reload_document::validate_arguments(&call.arguments).is_err()
                {
                    return Ok(None);
                }
                let path = {
                    let state = ctx.state.lock().unwrap_or_else(|p| p.into_inner());
                    if !state.recovery.required() {
                        return Ok(None);
                    }
                    state.current_path.clone()
                };
                return primary_scope::Scope::bind(owner, path.as_deref())
                    .map(Some)
                    .map_err(failed);
            }
        }
    }
    primary_scope::Scope::read(owner, ctx.authority.recovery.clone())
        .map(Some)
        .map_err(failed)
}
pub(super) fn startup(
    prepared: &Arc<primary_files::Prepared>,
    raw: Option<PathBuf>,
    policy: WebCredentialPersistence,
) -> Result<op_editor_core::EditorState> {
    let base = super::startup_editor_for_web_canvas_with_policy(None, policy)?;
    match raw {
        Some(raw) => {
            let mut next = prepared
                .load_editor(&raw)
                .map_err(|error| WebCanvasError::Document(error.to_string()))?;
            super::doc_routes::preserve_web_canvas_preferences(&base, &mut next);
            super::doc_routes::set_file_name_display(&mut next, &raw);
            next.editor_ui.touch_recent_file(
                raw.to_string_lossy().into_owned(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0),
            );
            prepared.verify_source().map_err(failed)?;
            Ok(next)
        }
        None => Ok(base),
    }
}
