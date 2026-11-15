//! Real managed MCP requests must not acknowledge an unsaved command.

use super::*;
use crate::web_canvas_server::{connection, ConnAuthority, SseHub, WebCanvasState};
use op_editor_core::{Locale, PenNodeExt};
use serde_json::{json, Value};
use std::{
    io::{Cursor, Read, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "op-managed-commit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn canvas(&self) -> WebCanvasState {
        let mut editor = EditorState::new();
        editor.editor_ui.preserve_authored_geometry = true;
        let path = self.path("primary.op");
        crate::doc_io::save_to_path(&editor, &path).unwrap();
        let mut editor = crate::doc_io::load_editor_state(&path, Locale::EnUs).unwrap();
        editor.editor_ui.file_name_display = Some("Primary display name.op".into());
        WebCanvasState::new_with_path(editor, 3100, Some(path))
    }
    fn blocked(&self) -> PathBuf {
        let path = self.path("blocked.op");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("keep"), b"prior destination").unwrap();
        path
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Stream {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
}
impl Read for Stream {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.input.read(bytes)
    }
}
impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn request(tool: &str, arguments: Value) -> String {
    json!({"jsonrpc":"2.0","id":17,"method":"tools/call",
        "params":{"name":tool,"arguments":arguments}})
    .to_string()
}

fn call(
    state: &Mutex<WebCanvasState>,
    hub: &SseHub,
    mode: ServeMode,
    tool: &str,
    arguments: Value,
) -> Value {
    let body = request(tool, arguments);
    let frame = format!(
        "POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
        body.len(),
    );
    let mut stream = Stream {
        input: Cursor::new(frame.into_bytes()),
        output: Vec::new(),
    };
    connection::serve_one_in_mode(&mut stream, state, hub, mode).unwrap();
    let wire = String::from_utf8(stream.output).unwrap();
    assert!(wire.starts_with("HTTP/1.1 200 OK"), "{wire}");
    serde_json::from_str(wire.split_once("\r\n\r\n").unwrap().1).unwrap()
}

fn accepted(response: &Value) {
    assert_ne!(response["result"]["isError"], true, "{response}");
    assert_eq!(response["id"], 17);
}

fn insert() -> Value {
    json!({"kind":"rect","name":"Original","x":"0","y":"0","width":"12","height":"8"})
}

fn assert_saved(state: &Mutex<WebCanvasState>) {
    let state = state.lock().unwrap();
    let saved =
        crate::doc_io::load_editor_state(state.current_path.as_ref().unwrap(), Locale::EnUs)
            .unwrap();
    assert_eq!(saved.doc, state.editor.doc);
    assert_eq!(
        op_pen_loader::EditorMeta::from_state(&saved),
        op_pen_loader::EditorMeta::from_state(&state.editor)
    );
}

#[test]
fn managed_primary_commands_are_saved_before_the_success_tick() {
    let files = Files::new();
    let state = Mutex::new(files.canvas());
    let hub = SseHub::default();
    let ticks = hub.subscribe();
    for (index, (tool, args)) in [
        ("insert_node", insert()),
        (
            "update_node",
            json!({"node_id":"n1","data":{"x":11.899999618530273}}),
        ),
        ("undo", json!({})),
        ("redo", json!({})),
        ("delete_node", json!({"node_id":"n1"})),
        ("undo", json!({})),
    ]
    .into_iter()
    .enumerate()
    {
        accepted(&call(&state, &hub, ServeMode::Managed, tool, args));
        assert_saved(&state);
        assert_eq!(state.lock().unwrap().version, index as u64 + 1);
        assert_eq!(ticks.pending().unwrap().version, index as u64 + 1);
    }
    let state = state.lock().unwrap();
    assert_eq!(
        state.editor.doc.children[0].base().x.unwrap().to_bits(),
        "11.899999618530273".parse::<f64>().unwrap().to_bits()
    );
}

#[test]
fn managed_failed_save_restores_document_history_clipboard_and_metadata() {
    let files = Files::new();
    let state = Mutex::new(files.canvas());
    let hub = SseHub::default();
    for (tool, args) in [
        ("insert_node", insert()),
        ("insert_node", insert()),
        ("undo", json!({})),
        ("set_selection", json!({"node_id":"n1"})),
        ("copy_selected", json!({})),
    ] {
        accepted(&call(&state, &hub, ServeMode::Managed, tool, args));
    }
    let good = state.lock().unwrap().current_path.clone().unwrap();
    let good_bytes = std::fs::read(&good).unwrap();
    let blocked = files.blocked();
    state.lock().unwrap().current_path = Some(blocked.clone());
    let ticks = hub.subscribe();
    let before = state.lock().unwrap().editor.clone();
    let version = state.lock().unwrap().version;
    for (tool, args) in [
        ("undo", json!({})),
        ("redo", json!({})),
        ("cut_selected", json!({})),
        ("update_node", json!({"node_id":"n1","x":"77"})),
    ] {
        let response = call(&state, &hub, ServeMode::Managed, tool, args);
        assert_eq!(response["result"]["isError"], true, "{tool}: {response}");
        assert!(
            response.to_string().contains("managed primary save failed"),
            "{response}"
        );
        assert!(
            response.to_string().contains("command rolled back"),
            "{response}"
        );
        let state = state.lock().unwrap();
        assert_eq!(state.editor.doc, before.doc, "{tool}");
        assert_eq!(state.editor.history.past, before.history.past, "{tool}");
        assert_eq!(state.editor.history.future, before.history.future, "{tool}");
        assert_eq!(
            state.editor.snapshot_for_history(),
            before.snapshot_for_history(),
            "{tool}"
        );
        assert_eq!(state.editor.clipboard, before.clipboard, "{tool}");
        assert_eq!(state.editor.selection, before.selection, "{tool}");
        assert_eq!(state.editor.viewport, before.viewport, "{tool}");
        assert_eq!(state.editor.tool, before.tool, "{tool}");
        assert_eq!(
            state.editor.editor_ui.file_name_display,
            before.editor_ui.file_name_display
        );
        assert_eq!(
            state.editor.ui.variables.active_theme,
            before.ui.variables.active_theme
        );
        assert_eq!(
            state.editor.document_generation(),
            before.document_generation()
        );
        assert_eq!(state.editor.document_revision(), before.document_revision());
        assert_eq!(state.editor.saved_revision(), before.saved_revision());
        assert_eq!(state.editor.is_dirty(), before.is_dirty());
        assert_eq!(
            op_pen_loader::EditorMeta::from_state(&state.editor),
            op_pen_loader::EditorMeta::from_state(&before)
        );
        assert_eq!(state.current_path.as_ref(), Some(&blocked));
        assert_eq!(state.version, version);
        assert!(
            ticks.pending().is_none(),
            "failed save must not publish a success tick"
        );
    }
    assert_eq!(std::fs::read(good).unwrap(), good_bytes);
    assert_eq!(
        std::fs::read(blocked.join("keep")).unwrap(),
        b"prior destination"
    );
}

#[test]
fn managed_failed_cut_keeps_actor_ui_clipboard_and_live_ui_isolated() {
    let files = Files::new();
    let state = Mutex::new(files.canvas());
    let hub = SseHub::default();
    for _ in 0..2 {
        accepted(&call(
            &state,
            &hub,
            ServeMode::Managed,
            "insert_node",
            insert(),
        ));
    }
    accepted(&call(
        &state,
        &hub,
        ServeMode::Managed,
        "set_selection",
        json!({"node_id":"n2"}),
    ));
    accepted(&call(
        &state,
        &hub,
        ServeMode::Managed,
        "set_selection",
        json!({"agentId":"actor-a","node_id":"n1"}),
    ));
    accepted(&call(
        &state,
        &hub,
        ServeMode::Managed,
        "copy_selected",
        json!({"agentId":"actor-a"}),
    ));
    accepted(&call(
        &state,
        &hub,
        ServeMode::Managed,
        "set_selection",
        json!({"agentId":"actor-b","node_id":"n2"}),
    ));
    let good = state.lock().unwrap().current_path.clone().unwrap();
    let blocked = files.blocked();
    // Keep the same document/context key while failing rename: replace the
    // bound file with a directory, without changing its primary authority.
    std::fs::remove_file(&good).unwrap();
    std::fs::rename(blocked, &good).unwrap();
    let before = state.lock().unwrap().editor.clone();
    let response = call(
        &state,
        &hub,
        ServeMode::Managed,
        "cut_selected",
        json!({"agentId":"actor-a"}),
    );
    assert_eq!(response["result"]["isError"], true, "{response}");
    {
        let mut state = state.lock().unwrap();
        assert_eq!(state.editor.doc, before.doc);
        assert_eq!(state.editor.selection, before.selection);
        let contexts = std::sync::Arc::clone(&state.agent_contexts);
        for (actor, selected, clipboard_len) in [("actor-a", "n1", 1), ("actor-b", "n2", 0)] {
            mcp_serve::agent_context::with_agent_context(
                &contexts,
                &good,
                &mut state.editor,
                &request("get_selection", json!({"agentId":actor})),
                mcp_serve::tool_profile::McpAccessProfile::UNRESTRICTED,
                |editor| {
                    assert_eq!(editor.selection.anchor.as_str(), selected);
                    assert_eq!(editor.clipboard.len(), clipboard_len);
                    if clipboard_len != 0 {
                        assert_eq!(editor.clipboard[0].base().id, "n1");
                    }
                    Ok(None)
                },
            )
            .unwrap();
        }
        assert_eq!(state.editor.selection, before.selection);
        assert_eq!(state.editor.clipboard, before.clipboard);
        assert_eq!(state.current_path.as_ref(), Some(&good));
    }
}

#[test]
fn managed_view_commands_and_rejected_edits_do_not_attempt_to_save() {
    let files = Files::new();
    let state = Mutex::new(files.canvas());
    let hub = SseHub::default();
    accepted(&call(
        &state,
        &hub,
        ServeMode::Managed,
        "insert_node",
        insert(),
    ));
    state.lock().unwrap().current_path = Some(files.blocked());
    let version = state.lock().unwrap().version;
    accepted(&call(
        &state,
        &hub,
        ServeMode::Managed,
        "set_selection",
        json!({"agentId":"actor","node_id":"n1"}),
    ));
    accepted(&call(
        &state,
        &hub,
        ServeMode::Managed,
        "get_viewport",
        json!({"agentId":"actor"}),
    ));
    let rejected = call(
        &state,
        &hub,
        ServeMode::Managed,
        "update_node",
        json!({"node_id":"missing","x":"4"}),
    );
    assert_eq!(rejected["result"]["isError"], true);
    assert!(!rejected.to_string().contains("save failed"));
    assert_eq!(state.lock().unwrap().version, version);
}

#[test]
fn local_and_unbound_managed_commands_retain_their_existing_commit_contract() {
    let files = Files::new();
    for mode in [ServeMode::Local, ServeMode::Managed] {
        let mut canvas = files.canvas();
        canvas.current_path = if mode == ServeMode::Local {
            Some(files.blocked())
        } else {
            None
        };
        let state = Mutex::new(canvas);
        accepted(&call(
            &state,
            &SseHub::default(),
            mode,
            "insert_node",
            insert(),
        ));
        let state = state.lock().unwrap();
        assert_eq!(state.editor.doc.children.len(), 1);
        assert_eq!(state.version, 1);
    }
}

#[test]
fn managed_primary_handoff_refuses_before_touching_new_authority_or_context() {
    let files = Files::new();
    let mut canvas = files.canvas();
    let old = canvas.current_path.clone().unwrap();
    canvas.current_path = Some(files.path("new-primary.op"));
    let before = canvas.editor.doc.clone();
    let authority = ConnAuthority::new(&canvas);
    let state = Mutex::new(canvas);
    let hub = SseHub::default();
    let ticks = hub.subscribe();
    let ctx = ConnCtx {
        state: &state,
        authority: &authority,
        hub: &hub,
        mode: ServeMode::Managed,
        mcp_profile: mcp_serve::tool_profile::McpAccessProfile::UNRESTRICTED,
        rest_identity: None,
        write_barrier: None,
    };
    let response = dispatch(
        &ctx,
        &request("insert_node", insert()),
        ctx.mcp_profile,
        Some(&old),
        &old,
        false,
        true,
    )
    .unwrap();
    assert!(response.contains("primary changed"), "{response}");
    let state = state.lock().unwrap();
    assert_eq!(state.editor.doc, before);
    assert_eq!(state.version, 0);
    assert!(state.editor.history.past.is_empty());
    assert!(!state.current_path.as_ref().unwrap().exists());
    assert!(ticks.pending().is_none());
}
