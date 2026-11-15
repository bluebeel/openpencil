//! Bounded process-shared catalog serialization, independent of document data.
use super::tool_profile::McpAccessProfile;
use super::McpToolCatalog;
use std::sync::OnceLock;

pub(super) struct Parts {
    pub(super) before_kits: String,
    pub(super) after_kits: String,
}

pub(super) fn for_profile(profile: McpAccessProfile, debug: bool) -> &'static Parts {
    // Four profile bits and one debug bit: at most32 immutable small entries,
    // never one cache per file/client/agent. No document or user paths retained.
    static CACHE: [OnceLock<Parts>; 32] = [const { OnceLock::new() }; 32];
    let key = usize::from(profile.deny_unshareable_tools)
        | usize::from(profile.scopes.can_read()) << 1
        | usize::from(profile.scopes.can_write()) << 2
        | usize::from(profile.catalog == McpToolCatalog::Lean) << 3
        | usize::from(debug && cfg!(feature = "mcp-debug-tools")) << 4;
    CACHE[key].get_or_init(|| {
        let list = |schemas: Vec<&str>| {
            schemas
                .into_iter()
                .filter(|schema| {
                    super::tool_profile::schema_name(schema).is_none_or(|name| profile.lists(&name))
                })
                .map(super::schema_with_agent_context)
                .collect::<Vec<_>>()
                .join(",")
        };
        let before_kits = if profile.catalog == McpToolCatalog::Lean {
            list(
                super::super::lean_profile::lean_tool_schemas()
                    .iter()
                    .map(String::as_str)
                    .collect(),
            )
        } else {
            list(super::super::schemas::TOOL_SCHEMAS.to_vec())
        };
        #[cfg(feature = "mcp-debug-tools")]
        let after_kits = if debug && profile.catalog != McpToolCatalog::Lean {
            list(super::super::schemas::DEBUG_TOOL_SCHEMAS.to_vec())
        } else {
            String::new()
        };
        #[cfg(not(feature = "mcp-debug-tools"))]
        let after_kits = String::new();
        Parts {
            before_kits,
            after_kits,
        }
    })
}
