//! Profiled tool dispatch; public live snapshots stay distinct from a locked
//! file authority whose allocator and durable commit are owned by one request.

use super::*;

/// A live snapshot or custom applier does not prove sequential-ID prediction
/// or durable file authority. Existing tools keep their ordinary behavior.
pub fn process_message_with_applier_profiled<F>(
    state: &mut EditorState,
    line: &str,
    profile: tool_profile::McpAccessProfile,
    mut apply: F,
) -> Result<Option<String>, McpServeError>
where
    F: FnMut(&str, &mut EditorState, &EditorCommand) -> bool,
{
    dispatch(
        state,
        line,
        profile,
        false,
        false,
        |tool, state, action| match action {
            AuthorityAction::Command(command) => apply(tool, state, command),
            _ => false,
        },
    )
}

/// Only file-backed hosts holding their complete writer/actor authority may
/// opt in. Their applier must use the sequential document allocator and finish
/// atomic persistence or rollback before returning success. Desktop UI-thread
/// snapshots and caller-provided arbitrary appliers must keep the public API.
pub(crate) fn process_message_with_bound_authority_profiled<F>(
    state: &mut EditorState,
    line: &str,
    profile: tool_profile::McpAccessProfile,
    bound_authority: bool,
    mut apply: F,
) -> Result<Option<String>, McpServeError>
where
    F: FnMut(&str, &mut EditorState, &EditorCommand) -> bool,
{
    dispatch(
        state,
        line,
        profile,
        bound_authority,
        false,
        |tool, state, action| match action {
            AuthorityAction::Command(command) => apply(tool, state, command),
            _ => false,
        },
    )
}

/// Bound hosts explicitly distinguish command apply from durable commit of an
/// already-executed atomic edit. Commit false must mean no durable write was
/// acknowledged; uncertain IO/panics trip recovery and are never retried.
pub(crate) fn process_message_with_bound_authority_actions<F>(
    state: &mut EditorState,
    line: &str,
    profile: tool_profile::McpAccessProfile,
    bound_authority: bool,
    apply: F,
) -> Result<Option<String>, McpServeError>
where
    F: FnMut(&str, &mut EditorState, AuthorityAction<'_>) -> bool,
{
    dispatch(state, line, profile, bound_authority, true, apply)
}

fn dispatch<F>(
    state: &mut EditorState,
    line: &str,
    profile: tool_profile::McpAccessProfile,
    bound_authority: bool,
    live_atomic: bool,
    mut apply: F,
) -> Result<Option<String>, McpServeError>
where
    F: FnMut(&str, &mut EditorState, AuthorityAction<'_>) -> bool,
{
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // MCP handshake / discovery methods short-circuit the tool dispatcher.
    match sniff_method(trimmed).as_deref() {
        Some("initialize") => {
            return Ok(sniff_id_raw(trimmed).map(|id| initialize_response(&id)));
        }
        Some("tools/list") => {
            return Ok(sniff_id_raw(trimmed)
                .map(|id| tools_list_response(&id, state, debug_tools_enabled(), profile)));
        }
        Some("notifications/initialized") | Some("initialized") => {
            return Ok(None); // notification — no response required
        }
        Some("ping") => {
            return Ok(sniff_id_raw(trimmed)
                .map(|id| ping_response(&id, headless_token_from_env().as_deref())));
        }
        _ => {}
    }
    // Fall through: tools/call or legacy direct dispatch. The
    // registry snapshots `state` at build time, so it no longer
    // borrows it once the applier closure mutates it.
    let call = op_mcp::parse_tool_call(trimmed);
    // Enforcement, ahead of the registry build and therefore ahead of the
    // tool's own argument parsing: a denied tool never sees the path it was
    // asked to open.
    if let Some(call) = call.as_ref() {
        if let Some(refusal) = profile.refuse_call(&call.tool, &call.arguments) {
            // The ordinary tools/call error envelope (`isError:true`) with
            // the originating id, so a client sees a refusal it can read
            // rather than a transport failure that would drop the session.
            return Ok(Some(op_mcp::tool_response_to_json(
                &op_mcp::ToolResponse::Err {
                    id: call.id.clone(),
                    code: op_mcp::ToolErrorCode::ToolFailed,
                    message: refusal.message(&call.tool),
                },
            )));
        }
    }
    if bound_authority && live_atomic {
        if let Some(call) = call.as_ref().filter(|call| call.tool == "batch_tools") {
            if let Some(tool) =
                atomic_tools::try_execute(state, &call.arguments, profile, |state, action| {
                    apply("batch_tools", state, action)
                })?
            {
                let mut registry = op_mcp::ToolRegistry::default();
                registry.register(tool);
                return process_tool_message_with_registry(&registry, line, |_, _| false);
            }
        }
    }
    let requested_tool = call.as_ref().map(|call| call.tool.as_str());
    let arguments = call.as_ref().map(|call| &call.arguments);
    let mut registry = if bound_authority {
        registry::rebuild_registry_with_authority(state, requested_tool, profile, arguments, true)
    } else {
        rebuild_registry(state, requested_tool, profile, arguments)
    };
    if requested_tool == Some("batch_tools") {
        if let Some(arguments) = arguments {
            if bound_authority || atomic_tools::read_only(arguments) {
                registry.register(atomic_tools::prepare(
                    state,
                    arguments,
                    profile,
                    bound_authority,
                ));
            }
        }
    }
    process_tool_message_with_registry(&registry, line, |tool_name, cmd| {
        apply(tool_name, state, AuthorityAction::Command(cmd))
    })
}
