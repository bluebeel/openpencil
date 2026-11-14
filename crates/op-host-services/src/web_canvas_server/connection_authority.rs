//! Immutable connection policy beside the document writer lock.

use super::*;

pub(super) struct ConnAuthority {
    pub(super) routing: std::sync::Mutex<BoundRoute>,
    pub(super) recovery: crate::document_recovery::DocumentRecovery,
    pub(super) managed_groups: super::managed_group_commit::Groups,
    pub(super) agent_contexts: Arc<crate::mcp_serve::agent_context::AgentContexts>,
    pub(super) managed_lifecycle_token: Option<String>,
    pub(super) allow_origins: Vec<String>,
}

impl ConnAuthority {
    /// Production captures this after startup policy is installed, once per
    /// daemon/tenant. Request admission never waits for document mutation.
    pub(super) fn new(state: &WebCanvasState) -> Self {
        Self {
            routing: std::sync::Mutex::new(BoundRoute::capture(state)),
            recovery: state.recovery.clone(),
            managed_groups: super::managed_group_commit::Groups::default(),
            agent_contexts: Arc::clone(&state.agent_contexts),
            managed_lifecycle_token: state.managed_token.clone(),
            allow_origins: state.allow_origins.clone(),
        }
    }
}

#[derive(Clone)]
pub(super) struct BoundRoute {
    pub path: Option<PathBuf>,
    pub document: PathBuf,
    pub epoch: u64,
    pub generation: u64,
}
impl BoundRoute {
    fn capture(state: &WebCanvasState) -> Self {
        Self {
            path: state.current_path.clone(),
            document: state
                .current_path
                .as_ref()
                .map(|path| path.canonicalize().unwrap_or_else(|_| path.clone()))
                .unwrap_or_else(|| PathBuf::from("live://canvas/current")),
            epoch: state.recovery.epoch(),
            generation: state.editor.document_generation(),
        }
    }
}
impl ConnAuthority {
    pub(super) fn bound_route(&self) -> Option<BoundRoute> {
        let route = self
            .routing
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        (!self.recovery.required() && route.epoch == self.recovery.epoch()).then(|| route.clone())
    }
    // Called only while the caller holds this authority's state lock. A
    // replacement invalidates the cached epoch before another group can use it.
    pub(super) fn refresh_route(&self, state: &WebCanvasState) {
        let mut route = self
            .routing
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if route.epoch != state.recovery.epoch() || route.path != state.current_path {
            *route = BoundRoute::capture(state);
        }
    }
}
