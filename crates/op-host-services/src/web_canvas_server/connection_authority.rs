//! Immutable connection policy beside the document writer lock.

use super::*;

pub(super) struct ConnAuthority {
    pub(super) agent_contexts: Arc<crate::mcp_serve::agent_context::AgentContexts>,
    pub(super) managed_lifecycle_token: Option<String>,
    pub(super) allow_origins: Vec<String>,
}

impl ConnAuthority {
    /// Production captures this after startup policy is installed, once per
    /// daemon/tenant. Request admission never waits for document mutation.
    pub(super) fn new(state: &WebCanvasState) -> Self {
        Self {
            agent_contexts: Arc::clone(&state.agent_contexts),
            managed_lifecycle_token: state.managed_token.clone(),
            allow_origins: state.allow_origins.clone(),
        }
    }
}
