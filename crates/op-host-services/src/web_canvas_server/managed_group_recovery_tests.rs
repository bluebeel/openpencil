//! Real save/capture/HTTP controls. Prepared for root's single host test build.
use super::*;
use crate::web_canvas_server::{connection, doc_routes, export_routes, handle_web_canvas_request};
use std::io::{Cursor, Read, Write};

fn edit(ctx: &ConnCtx<'_>, value: i32) -> Result<String, WebCanvasError> {
    let line = line(0, value);
    let admission = ctx
        .authority
        .agent_contexts
        .try_admit(&line, PROFILE)
        .unwrap();
    let path = ctx.state.lock().unwrap().current_path.clone().unwrap();
    dispatch(ctx, &line, PROFILE, Some(&path), &path, &admission, true).unwrap()
}

fn saved_x(path: &Path) -> f64 {
    let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    value["pages"][0]["children"][1]["x"]
        .as_f64()
        .unwrap_or(0.0)
}

#[test]
fn post_rename_unknown_outcome_is_fenced_then_explicitly_reloads_without_replay() {
    let files = Files::new();
    let state = files.state(false);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    let ctx = ctx(&state, &authority, &hub);
    authority.managed_groups.fault.store(2, Ordering::Release);
    assert!(edit(&ctx, 41).is_err()); // No ToolFailed rejection for an uncertain write.
    assert_eq!(saved_x(&files.path()), 41.0); // The real atomic rename happened.
    {
        let mut state = state.lock().unwrap();
        assert!(state.recovery.required());
        assert_eq!(
            handle_web_canvas_request("GET", "/api/mcp/document", "", &mut state).status,
            "503 Service Unavailable"
        );
        assert_eq!(
            handle_web_canvas_request("POST", "/api/file/reload", "{}", &mut state).status,
            "200 OK"
        );
        assert!(!state.recovery.required());
        assert_eq!(state.editor.history.past.len(), 0);
        assert_eq!(state.editor.document_revision(), 0);
    }
    assert_eq!(saved_x(&files.path()), 41.0); // Recovery performs no write or replay.
    authority.managed_groups.fault.store(0, Ordering::Release);
    assert!(edit(&ctx, 42).is_ok());
    let mut state = state.lock().unwrap();
    assert_eq!(state.editor.history.past.len(), 1);
    assert!(state.editor.undo());
    let node = op_editor_core::walkers::find_node(
        &state.editor.doc.pages.as_ref().unwrap()[0].children,
        &NodeId::new("n0"),
    )
    .unwrap();
    assert_eq!(node.base().x, Some(41.0));
}

#[test]
fn pre_rename_unknown_outcome_reloads_the_old_file_without_reexecuting_the_edit() {
    let files = Files::new();
    let state = files.state(false);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    authority.managed_groups.fault.store(1, Ordering::Release);
    let before = std::fs::read(files.path()).unwrap();
    assert!(edit(&ctx(&state, &authority, &hub), 77).is_err());
    let mut state = state.lock().unwrap();
    assert!(state.recovery.required());
    state.reload_authoritative_file().unwrap();
    assert_eq!(std::fs::read(files.path()).unwrap(), before);
    assert_eq!(state.editor.history.past.len(), 0);
    assert_eq!(
        state.editor.doc.pages.as_ref().unwrap()[0].children[1]
            .base()
            .x,
        Some(0.0)
    );
}

#[test]
fn actual_stale_capture_rollback_failure_requires_recovery() {
    let files = Files::new();
    let state = files.state(true); // Existing save contract fails at this directory.
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    authority.managed_groups.fault.store(3, Ordering::Release);
    assert!(edit(&ctx(&state, &authority, &hub), 55).is_err());
    let mut state = state.lock().unwrap();
    assert!(state.recovery.required());
    assert!(state.reload_authoritative_file().is_err());
    assert!(state.recovery.required());
    assert_eq!(
        std::fs::read(files.path().join("keep")).unwrap(),
        b"prior authority"
    );
}

#[test]
fn failed_recovery_keeps_binding_fenced_and_can_retry_only_the_load() {
    let files = Files::new();
    let state = files.state(false);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    authority.managed_groups.fault.store(2, Ordering::Release);
    assert!(edit(&ctx(&state, &authority, &hub), 61).is_err());
    let authoritative = std::fs::read(files.path()).unwrap();
    std::fs::write(files.path(), b"{invalid saved source").unwrap();
    let mut state = state.lock().unwrap();
    let version = state.version;
    assert_eq!(
        handle_web_canvas_request("POST", "/api/file/reload", "{}", &mut state).status,
        "503 Service Unavailable"
    );
    assert!(state.recovery.required());
    assert_eq!(state.current_path.as_ref(), Some(&files.path()));
    assert_eq!(state.version, version);
    assert_eq!(
        handle_web_canvas_request("GET", "/api/mcp/document", "", &mut state).status,
        "503 Service Unavailable"
    );
    std::fs::write(files.path(), &authoritative).unwrap();
    state.reload_authoritative_file().unwrap();
    assert!(!state.recovery.required());
    assert_eq!(std::fs::read(files.path()).unwrap(), authoritative);
    assert_eq!(state.editor.history.past.len(), 0);
}

#[test]
fn direct_rest_helpers_and_state_commands_cannot_bypass_the_fence() {
    let files = Files::new();
    let state = files.state(false);
    let before = std::fs::read(files.path()).unwrap();
    let mut state = state.lock().unwrap();
    state.recovery.trip();
    let before_ui = state.editor.selection.anchor.clone();
    assert_eq!(
        export_routes::save_current_file("{}", &mut state).status,
        "503 Service Unavailable"
    );
    assert_eq!(
        export_routes::export_raster_download("{}", &state).status,
        "503 Service Unavailable"
    );
    assert_eq!(
        export_routes::export_pdf_download("{}", &state).status,
        "503 Service Unavailable"
    );
    assert_eq!(
        doc_routes::unbind_current_file(&mut state).status,
        "503 Service Unavailable"
    );
    assert_eq!(
        doc_routes::open_recent_file("{}", &mut state).status,
        "503 Service Unavailable"
    );
    assert_eq!(
        doc_routes::apply_selection_sync(r#"{"selectedIds":["n0"]}"#, &mut state).status,
        "503 Service Unavailable"
    );
    assert!(state
        .apply_document_push(r#"{"document":{"version":"1.0.0","children":[]}}"#, None)
        .is_err());
    assert!(state.reset_document_guarded().is_err());
    assert!(state
        .replace_document(op_editor_core::EditorState::new().doc)
        .is_err());
    assert!(state
        .apply_gated(
            C::SetNodeName {
                node_id: NodeId::new("n0"),
                name: "bypass".into()
            },
            op_editor_core::CollabEditSource::Mcp
        )
        .is_err());
    assert_eq!(state.current_path.as_ref(), Some(&files.path()));
    assert_eq!(state.editor.selection.anchor, before_ui);
    assert_eq!(std::fs::read(files.path()).unwrap(), before);
}

struct Stream {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
}
impl Read for Stream {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.input.read(b)
    }
}
impl Write for Stream {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.output.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn http(
    state: &Mutex<WebCanvasState>,
    authority: &ConnAuthority,
    hub: &SseHub,
    path: &str,
    body: &str,
) -> (Result<bool, WebCanvasError>, String) {
    let frame = format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len());
    let mut stream = Stream {
        input: Cursor::new(frame.into_bytes()),
        output: Vec::new(),
    };
    let result = connection::serve_one_with_authority(
        &mut stream,
        state,
        hub,
        ServeMode::Managed,
        authority,
    );
    (result, String::from_utf8(stream.output).unwrap())
}
fn tool(name: &str, args: Value) -> String {
    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})
        .to_string()
}

#[test]
fn real_http_primary_alias_copy_and_preparsed_push_cannot_bypass_fenced_authority() {
    let files = Files::new();
    let state = files.state(false);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    let before = std::fs::read(files.path()).unwrap();
    state.lock().unwrap().recovery.trip();
    let body = tool(
        "update_node",
        json!({"agentId":"alias","filePath":files.path(),"nodeId":"n0","data":{"x":99}}),
    );
    let (outcome, wire) = http(&state, &authority, &hub, "/mcp", &body);
    assert!(outcome.is_err());
    assert!(wire.is_empty());
    let destination = files.0.join("copy.op");
    std::fs::write(&destination, b"copy must stay unchanged").unwrap();
    let body = tool(
        "save_document",
        json!({"agentId":"copy","filePath":destination}),
    );
    let (outcome, wire) = http(&state, &authority, &hub, "/mcp", &body);
    assert!(outcome.is_ok());
    assert!(wire.contains("document-recovery-required"));
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"copy must stay unchanged"
    );
    let (outcome, wire) = http(
        &state,
        &authority,
        &hub,
        "/api/mcp/document",
        r#"{"document":{"version":"1.0.0","children":[]}}"#,
    );
    assert!(outcome.is_ok());
    assert!(wire.starts_with("HTTP/1.1 503"));
    assert_eq!(std::fs::read(files.path()).unwrap(), before);
    assert!(state.lock().unwrap().recovery.required());
}

#[test]
fn unrelated_alternate_file_still_operates_while_primary_is_fenced() {
    let files = Files::new();
    let state = files.state(false);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    state.lock().unwrap().recovery.trip();
    let path = files.0.join("alternate.op");
    std::fs::write(&path,r#"{"version":"1.0.0","children":[{"type":"rectangle","id":"a","x":0,"width":10,"height":10}]}"#).unwrap();
    let body = tool(
        "update_node",
        json!({"agentId":"alternate","filePath":path,"nodeId":"a","data":{"x":17}}),
    );
    let (outcome, wire) = http(&state, &authority, &hub, "/mcp", &body);
    assert!(outcome.is_ok(), "{wire}");
    assert!(wire.starts_with("HTTP/1.1 200"));
    let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(value["children"][0]["x"], 17);
    assert!(state.lock().unwrap().recovery.required());
}
