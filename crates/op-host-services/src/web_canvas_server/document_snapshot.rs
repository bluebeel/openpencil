//! Share complete bootstrap encoding across clients of one document authority.
//! The public REST helper stays fresh for unversioned/custom EditorState users.

use super::{connection::ConnCtx, Result, WebCanvasError, WebCanvasState, WebReply};
use std::io::{IoSlice, Write};
use std::sync::Arc;

const MAX_RETAINED_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    epoch: u64,
    generation: u64,
    revision: u64,
    version: u64,
}

#[derive(Default)]
pub(super) struct Cache {
    entry: Option<(Key, Arc<Vec<u8>>)>,
}

struct Snapshot {
    document: Arc<Vec<u8>>,
    suffix: String,
}

impl Snapshot {
    fn capture(state: &mut WebCanvasState) -> std::result::Result<Self, WebReply> {
        if state.recovery.required() {
            return Err(super::managed_group_commit::recovery::refusal());
        }
        let operation = state.recovery.watch();
        let result = Self::capture_healthy(state);
        operation.complete();
        result
    }

    fn capture_healthy(state: &mut WebCanvasState) -> std::result::Result<Self, WebReply> {
        let cursor = state.change_cursor();
        let key = Key {
            epoch: state.recovery.epoch(),
            generation: state.editor.document_generation(),
            revision: state.editor.document_revision(),
            version: state.version,
        };
        // This entry belongs to this exact tenant's exclusively locked HTTP
        // authority. No process-global document keys or cloned Doc per reader.
        let document = match &state.bootstrap_snapshot.entry {
            Some((cached, bytes)) if *cached == key => Arc::clone(bytes),
            _ => {
                state.bootstrap_snapshot.entry = None;
                let bytes =
                    Arc::new(
                        serde_json::to_vec(&state.editor.doc).map_err(|error| WebReply {
                            status: "500 Internal Server Error",
                            body: crate::mcp_serve::rest_error_body(&error.to_string()),
                        })?,
                    );
                if bytes.capacity() <= MAX_RETAINED_BYTES {
                    state.bootstrap_snapshot.entry = Some((key, Arc::clone(&bytes)));
                }
                bytes
            }
        };
        // UI projection and cursor are always current even if a document-only
        // cache hit follows a selection/page/presentation context change.
        let scenario = state
            .editor
            .editor_ui
            .scenario
            .map(|scene| format!("\"{}\"", scene.as_str()))
            .unwrap_or_else(|| "null".into());
        let suffix = format!(
            ",\"version\":{},\"activePageIndex\":{},\"preserveAuthoredGeometry\":{},\"scenario\":{scenario},\"changeCursor\":\"{cursor}\"}}",
            state.version, state.editor.ui.active_page_index,
            state.editor.editor_ui.preserve_authored_geometry,
        );
        Ok(Self { document, suffix })
    }
}

pub(super) fn serve<S: Write>(
    stream: &mut S,
    ctx: &ConnCtx<'_>,
    origin: Option<&str>,
) -> Result<()> {
    let snapshot = {
        let mut state = ctx
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        Snapshot::capture(&mut state)
    };
    let snapshot = match snapshot {
        Ok(snapshot) => snapshot,
        Err(reply) => {
            crate::mcp_serve::write_mcp_http_response_with_origin(
                stream,
                reply.status,
                &reply.body,
                origin,
            )?;
            return Ok(());
        }
    };
    const PREFIX: &[u8] = b"{\"document\":";
    let length = PREFIX.len() + snapshot.document.len() + snapshot.suffix.len();
    let cors = origin
        .map(|origin| format!("Access-Control-Allow-Origin: {origin}\r\n"))
        .unwrap_or_default();
    let header = format!("HTTP/1.1 200 OK\r\n{cors}Access-Control-Allow-Methods: GET, POST, DELETE, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, mcp-session-id, X-OpenPencil-Token, Authorization\r\nAccess-Control-Expose-Headers: mcp-session-id\r\nmcp-session-id: openpencil\r\nCache-Control: no-store\r\nContent-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n");
    let mut parts = [
        IoSlice::new(header.as_bytes()),
        IoSlice::new(PREFIX),
        IoSlice::new(snapshot.document.as_slice()),
        IoSlice::new(snapshot.suffix.as_bytes()),
    ];
    let mut remaining = parts.as_mut_slice();
    // A slow reader pins only an Arc to its exact encoded revision. No socket
    // I/O or response-sized assembly takes place under the document mutex.
    while !remaining.is_empty() {
        match stream.write_vectored(remaining) {
            Ok(0) => {
                return Err(WebCanvasError::Transport(
                    "document snapshot: write zero".into(),
                ))
            }
            Ok(written) => IoSlice::advance_slices(&mut remaining, written),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(WebCanvasError::Transport(format!(
                    "document snapshot: {error}"
                )))
            }
        }
    }
    stream
        .flush()
        .map_err(|error| WebCanvasError::Transport(format!("document snapshot flush: {error}")))
}
