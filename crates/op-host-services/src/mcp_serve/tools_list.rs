//! The `tools/list` catalog response.
//!
//! Split out of the `mcp_serve` spine at the 800-line cap. It lives next to
//! `tool_profile` because the two are one decision: the profile says which
//! tools a deployment offers, and this is where that answer becomes the
//! catalog a client sees.

use op_editor_core::EditorState;

use super::lean_profile::lean_tool_schemas;
#[cfg(feature = "mcp-debug-tools")]
use super::schemas::DEBUG_TOOL_SCHEMAS;
use super::schemas::TOOL_SCHEMAS;
use super::tool_catalog::McpToolCatalog;
use super::tool_profile;

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
    // Discovery follows enforcement: a tool this profile would refuse is not
    // advertised, so a client never plans around one it cannot call.
    let listed =
        |schema: &str| tool_profile::schema_name(schema).is_none_or(|name| profile.lists(&name));
    // The lean catalog advertises its own derived schemas — never the kit
    // insert tools or debug tools, which it does not serve.
    if profile.catalog == McpToolCatalog::Lean {
        let entries: Vec<String> = lean_tool_schemas()
            .iter()
            .map(String::as_str)
            .filter(|schema| listed(schema))
            .map(schema_with_agent_context)
            .collect();
        return catalog_response(id_raw, &entries, state.is_none());
    }
    let mut entries: Vec<String> = TOOL_SCHEMAS
        .iter()
        .filter(|schema| listed(schema))
        .map(|s| schema_with_agent_context(s))
        .collect();
    if let Some(state) = state {
        entries.extend(
            op_mcp::element_tools::element_tool_schemas(state)
                .into_iter()
                .filter(|schema| listed(schema))
                .map(|schema| schema_with_agent_context(&schema)),
        );
    }
    #[cfg(not(feature = "mcp-debug-tools"))]
    let _ = debug_enabled;
    #[cfg(feature = "mcp-debug-tools")]
    if debug_enabled {
        entries.extend(
            DEBUG_TOOL_SCHEMAS
                .iter()
                .filter(|schema| listed(schema))
                .map(|s| schema_with_agent_context(s)),
        );
    }
    catalog_response(id_raw, &entries, state.is_none())
}

fn catalog_response(id_raw: &str, entries: &[String], fenced: bool) -> String {
    let recovery_meta = if fenced {
        r#","_meta":{"documentRecoveryRequired":true,"dynamicToolsOmitted":true}"#
    } else {
        ""
    };
    format!(
        r#"{{"jsonrpc":"2.0","id":{id_raw},"result":{{"tools":[{}]{recovery_meta}}}}}"#,
        entries.join(",")
    )
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
        "description": "Optional host UI context identifier. Isolates selection, viewport, active page and clipboard; document and undo history remain shared. Not an authentication credential."
    });
    value.to_string()
}
