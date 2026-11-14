//! The `tools/list` catalog response.
//!
//! Split out of the `mcp_serve` spine at the 800-line cap. It lives next to
//! `tool_profile` because the two are one decision: the profile says which
//! tools a deployment offers, and this is where that answer becomes the
//! catalog a client sees.

use op_editor_core::EditorState;

use super::tool_catalog::McpToolCatalog;
use super::tool_profile;
#[path = "tools_list_cache.rs"]
mod cache;

pub(super) fn tools_list_response(
    id_raw: &str,
    state: &EditorState,
    debug_enabled: bool,
    profile: tool_profile::McpAccessProfile,
) -> String {
    tools_list_response_inner(id_raw, Some(state), debug_enabled, profile)
}

/// Fenced authorities can still handshake and discover their static recovery
/// tool. Do not derive kit tools from a document whose write outcome is unknown.
/// Healthy discovery continues to use the full dynamic catalog above.
pub(crate) fn fenced_protocol_response(
    line: &str,
    profile: tool_profile::McpAccessProfile,
) -> Option<String> {
    match super::classify_stateless(line) {
        super::Stateless::Respond(response) => Some(response),
        super::Stateless::Swallow => Some(String::new()),
        super::Stateless::Ping(id) => Some(super::ping_response(
            &id,
            super::headless_token_from_env().as_deref(),
        )),
        super::Stateless::NeedsState
            if super::sniff_method(line).as_deref() == Some("tools/list") =>
        {
            Some(super::sniff_id_raw(line).map_or_else(String::new, |id| {
                tools_list_response_inner(&id, None, op_mcp::debug_tools_enabled(), profile)
            }))
        }
        super::Stateless::NeedsState => None,
    }
}

fn tools_list_response_inner(
    id_raw: &str,
    state: Option<&EditorState>,
    debug_enabled: bool,
    profile: tool_profile::McpAccessProfile,
) -> String {
    let parts = cache::for_profile(profile, debug_enabled);
    let mut dynamic = Vec::new();
    if profile.catalog != McpToolCatalog::Lean {
        if let Some(state) = state {
            dynamic.extend(
                op_mcp::element_tools::element_tool_schemas(state)
                    .into_iter()
                    .filter(|schema| {
                        tool_profile::schema_name(schema).is_none_or(|name| profile.lists(&name))
                    })
                    .map(|schema| schema_with_agent_context(&schema)),
            );
        }
    }
    // Only kit definitions depend on this document. Static schema parsing,
    // agent-context expansion and capability filtering are shared by clients.
    catalog_response(
        id_raw,
        &parts.before_kits,
        &dynamic,
        &parts.after_kits,
        state.is_none(),
    )
}

fn catalog_response(
    id_raw: &str,
    before: &str,
    dynamic: &[String],
    after: &str,
    fenced: bool,
) -> String {
    use std::fmt::Write;
    let capacity = before.len()
        + after.len()
        + dynamic.iter().map(String::len).sum::<usize>()
        + dynamic.len()
        + 128;
    let mut response = String::with_capacity(capacity);
    let _ = write!(
        response,
        r#"{{"jsonrpc":"2.0","id":{id_raw},"result":{{"tools":["#
    );
    let mut separator = "";
    for section in std::iter::once(before)
        .chain(dynamic.iter().map(String::as_str))
        .chain(std::iter::once(after))
    {
        if !section.is_empty() {
            response.push_str(separator);
            response.push_str(section);
            separator = ",";
        }
    }
    response.push(']');
    if fenced {
        response
            .push_str(r#","_meta":{"documentRecoveryRequired":true,"dynamicToolsOmitted":true}"#);
    }
    response.push_str("}}");
    response
}

// Discovery advertises the host extension without changing individual tools or
// widening any deployment's catalog/authorization profile.
fn schema_with_agent_context(schema: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(schema) else {
        return schema.to_owned();
    };
    value["inputSchema"]["properties"]["agentId"] = serde_json::json!({
        "type": "string", "minLength": 1, "maxLength": 96,
        "pattern": "^[A-Za-z0-9._:-]+$",
        "description": "Optional actor context identifier. Isolates selection, viewport, active page, clipboard and undo/redo ownership within one shared document/history store. Not an authentication credential."
    });
    value.to_string()
}
