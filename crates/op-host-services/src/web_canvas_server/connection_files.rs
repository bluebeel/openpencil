//! Local file routing keeps primary-source copies under the live authority.
use super::*;

pub(super) fn route(
    ctx: &ConnCtx<'_>,
    line: &str,
    current_path: Option<&std::path::Path>,
    document_key: &std::path::Path,
    mcp_profile: crate::mcp_serve::tool_profile::McpAccessProfile,
    admission: &crate::mcp_serve::agent_context::AgentAdmission<'_>,
    routing_epoch: u64,
) -> Result<Option<String>> {
    if mcp_profile.deny_unshareable_tools {
        return Ok(None);
    }
    let canonical_primary = current_path.map(|_| document_key);
    let state = ctx.state;
    let agent_contexts = &ctx.authority.agent_contexts;
    let routing_guard = crate::mcp_serve::resident_groups::RoutingGuard::new(
        &ctx.authority.recovery,
        routing_epoch,
    );
    if let Some(response) = crate::mcp_serve::resident_groups::dispatch(
        agent_contexts,
        admission,
        canonical_primary,
        line,
        mcp_profile,
        routing_guard,
    ) {
        return response.map(Some).map_err(Into::into);
    }
    let mut primary_save_uncertain = false;
    let routed = crate::mcp_serve::file_path::process_live_save_for_file_path_arg(
        canonical_primary,
        line,
        mcp_profile,
        |destination| {
            let mut guard = state.lock().unwrap_or_else(|p| p.into_inner());
            if guard.current_path.as_deref() != current_path {
                return Err(crate::mcp_serve::McpServeError::Document(
                    "managed primary changed while routing this save; no write was applied".into(),
                ));
            }
            guard
                .ensure_document_available()
                .map_err(|e| crate::mcp_serve::McpServeError::Document(e.to_string()))?;
            let fence = guard.recovery.clone();
            let response = crate::mcp_serve::agent_context::with_agent_context(
                &agent_contexts,
                document_key,
                &mut guard.editor,
                line,
                mcp_profile,
                |editor| {
                    let operation = fence.watch();
                    let saved = crate::doc_io::save_to_path(editor, destination).map_err(|error| {
                        crate::mcp_serve::McpServeError::Document(error.to_string())
                    });
                    operation.complete();
                    saved?;
                    Ok(None)
                },
            );
            if fence.required() {
                primary_save_uncertain = true;
                return Err(crate::mcp_serve::McpServeError::Io(
                    crate::document_recovery::REQUIRED.into(),
                ));
            }
            response.map(|_| ())
        },
    )?;
    if primary_save_uncertain {
        return Err(WebCanvasError::Transport(
            crate::document_recovery::REQUIRED.into(),
        ));
    }
    let routed = match routed {
        Some(response) => Some(response),
        None => crate::mcp_serve::file_path::process_message_for_file_path_arg_with_contexts(
            &agent_contexts,
            canonical_primary,
            line,
            mcp_profile,
        )?,
    };
    Ok(routed)
}
