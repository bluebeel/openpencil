//! Construct shared policy/ownership exactly once per host, never per agent.
use super::*;
impl WebCanvasState {
    pub(super) fn new_with_path_and_policy(
        editor: EditorState,
        port: u16,
        current_path: Option<PathBuf>,
        credential_persistence: WebCredentialPersistence,
    ) -> Self {
        let primary_files = primary_files::PrimaryFiles::supplied_editor(current_path.as_deref());
        Self::new_with_primary_files(
            editor,
            port,
            current_path,
            credential_persistence,
            primary_files,
        )
    }
    pub(super) fn new_with_primary_files(
        mut editor: EditorState,
        port: u16,
        current_path: Option<PathBuf>,
        credential_persistence: WebCredentialPersistence,
        primary_files: Arc<primary_files::PrimaryFiles>,
    ) -> Self {
        if !credential_persistence.server_persistence() {
            let _ = crate::web_credentials::remove_browser_owned_credentials(&mut editor);
        }
        let recovery = crate::document_recovery::DocumentRecovery::default();
        if !primary_files.verified() {
            recovery.trip();
        }
        Self {
            editor,
            recovery,
            primary_files,
            changefeed: Default::default(),
            bootstrap_snapshot: Default::default(),
            agent_contexts: Arc::new(crate::mcp_serve::agent_context::AgentContexts::default()),
            credential_persistence,
            current_path,
            version: 0,
            port,
            managed_token: None,
            allow_origins: Vec::new(),
            reset_consumed: false,
            auth_login_handle: None,
            collab: collab_state::WebCollabState::default(),
            mode: ServeMode::Local,
        }
    }
}
