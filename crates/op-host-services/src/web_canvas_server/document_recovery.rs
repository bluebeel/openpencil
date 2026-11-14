//! Explicit recovery of one bound authority. No failed command is replayed.
use super::super::{doc_routes, Result, WebCanvasError, WebCanvasState, WebReply};
use crate::document_recovery::REQUIRED;

pub(in crate::web_canvas_server) fn refusal() -> WebReply {
    WebReply {
        status: "503 Service Unavailable",
        body: serde_json::json!({
            "ok":false,"error":"document-recovery-required","message":REQUIRED,
            "recoveryRoute":"/api/file/reload","replay":false
        })
        .to_string(),
    }
}

impl WebCanvasState {
    pub(crate) fn ensure_document_available(&self) -> Result<()> {
        if self.recovery.required() {
            Err(WebCanvasError::Config(REQUIRED.into()))
        } else {
            Ok(())
        }
    }
    /// This is an explicit recovery, not the mount-time idempotent sync-reset.
    /// Uses only the existing bound path: no browser document/path is accepted.
    pub(crate) fn reload_authoritative_file(&mut self) -> Result<()> {
        let epoch = self.recovery.epoch();
        if !self.recovery.required() {
            return Ok(());
        }
        if !self.mode.allows_local_file_routes() {
            return Err(WebCanvasError::Config(
                "recovery requires a local bound file".into(),
            ));
        }
        self.collab_policy()
            .check(
                op_editor_core::CollabGateAction::ReplaceDocument,
                op_editor_core::CollabEditSource::ExternalSync,
            )
            .map_err(|reason| {
                WebCanvasError::Collab(super::super::DaemonMutationRefusal::Collab(reason))
            })?;
        let path = self.current_path.clone().ok_or_else(|| {
            WebCanvasError::Config(
                "recovery requires the original bound file; restart with that file".into(),
            )
        })?;
        let expected = Stamp::read(&path)?;
        let previous = std::mem::replace(&mut self.editor, op_editor_core::EditorState::new());
        doc_routes::preserve_web_canvas_preferences(&previous, &mut self.editor);
        drop(previous);
        // Stage the replacement without touching the uncertain authority. A
        // decode/stat/load failure retains the fence and original binding.
        let mut next = crate::mcp_serve::load_editor_state(&path)?;
        if expected != Stamp::read(&path)? {
            return Err(WebCanvasError::Io(
                "authoritative file changed during recovery; reload again".into(),
            ));
        }
        doc_routes::preserve_web_canvas_preferences(&self.editor, &mut next);
        doc_routes::set_file_name_display(&mut next, &path);
        // Human selection, clipboard, drafts, history and active page come from
        // the canonical load. AgentContexts stays owned by this daemon; each
        // subsequent actor scope prunes its saved ids against the loaded doc.
        self.editor = next;
        self.version = self.version.saturating_add(1);
        if !self.recovery.recovered(epoch) {
            return Err(WebCanvasError::Config(REQUIRED.into()));
        }
        Ok(())
    }
}

#[derive(PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    identity: (u64, u64),
}
impl Stamp {
    fn read(path: &std::path::Path) -> Result<Self> {
        let metadata = std::fs::metadata(path).map_err(|e| WebCanvasError::Io(e.to_string()))?;
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        };
        #[cfg(not(unix))]
        let identity = (0, 0);
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            identity,
        })
    }
}

pub(in crate::web_canvas_server) fn reload_reply(state: &mut WebCanvasState) -> WebReply {
    match state.reload_authoritative_file() {
        Ok(()) => WebReply { status:"200 OK", body:serde_json::json!({
            "ok":true,"version":state.version,"reloaded":true,"replay":false
        }).to_string() },
        Err(error) => WebReply { status:"503 Service Unavailable", body:serde_json::json!({
            "ok":false,"error":"document-recovery-required","message":error.to_string(),"replay":false
        }).to_string() }
    }
}
