//! Real file-backed MCP calls must acknowledge undoable, durable edits.

use super::*;
use op_editor_core::PenNodeExt;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "op-mcp-undo-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
    fn create(&self, name: &str) -> PathBuf {
        let mut state = EditorState::new();
        state.editor_ui.preserve_authored_geometry = true;
        let path = self.0.join(format!("{name}.op"));
        save_editor_state(&state, &path).unwrap();
        path
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn insert_args() -> serde_json::Value {
    serde_json::json!({"kind":"rect","name":"Original","x":"0","y":"0","width":"12","height":"8"})
}

fn call(state: &mut EditorState, primary: &Path, tool: &str, args: serde_json::Value) {
    let response = response(state, primary, tool, args);
    assert_ne!(response["result"]["isError"], true, "{tool}: {response}");
}

fn response(
    state: &mut EditorState,
    primary: &Path,
    tool: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    let request = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":tool,"arguments":args}})
    .to_string();
    let response = process_message(state, primary, &request).unwrap().unwrap();
    serde_json::from_str(&response).unwrap()
}

#[test]
fn primary_raw_crud_round_trips_through_durable_undo_redo() {
    let files = Files::new();
    let path = files.create("primary");
    let mut state = load_editor_state(&path).unwrap();
    let original = state.doc.clone();
    call(&mut state, &path, "insert_node", insert_args());
    assert_eq!(state.history.past.len(), 1);
    assert_eq!(load_editor_state(&path).unwrap().doc, state.doc);
    let inserted = state.doc.clone();
    call(
        &mut state,
        &path,
        "update_node",
        serde_json::json!({
        "node_id":"n1","x":"43","y":"27","name":"Changed"}),
    );
    assert_eq!(state.history.past.len(), 2);
    let changed = state.doc.clone();
    assert_ne!(changed, inserted);
    call(&mut state, &path, "undo", serde_json::json!({}));
    assert_eq!(load_editor_state(&path).unwrap().doc, inserted);
    call(&mut state, &path, "undo", serde_json::json!({}));
    assert_eq!(load_editor_state(&path).unwrap().doc, original);
    call(&mut state, &path, "redo", serde_json::json!({}));
    call(&mut state, &path, "redo", serde_json::json!({}));
    assert_eq!(load_editor_state(&path).unwrap().doc, changed);
    call(
        &mut state,
        &path,
        "delete_node",
        serde_json::json!({"node_id":"n1"}),
    );
    assert_eq!(state.history.past.len(), 3);
    call(&mut state, &path, "undo", serde_json::json!({}));
    assert_eq!(load_editor_state(&path).unwrap().doc, changed);
}

#[test]
fn resident_raw_edits_keep_independent_durable_history_per_file() {
    let files = Files::new();
    let first = files.create("first");
    let second = files.create("second");
    let primary = files.0.join("unused-primary.op");
    let mut state = EditorState::new();
    for path in [&first, &second] {
        let mut args = insert_args();
        args["filePath"] = serde_json::json!(path);
        call(&mut state, &primary, "insert_node", args);
        assert_eq!(load_editor_state(path).unwrap().doc.children.len(), 1);
    }
    call(
        &mut state,
        &primary,
        "undo",
        serde_json::json!({"filePath":first}),
    );
    assert!(load_editor_state(&first).unwrap().doc.children.is_empty());
    assert_eq!(load_editor_state(&second).unwrap().doc.children.len(), 1);
    call(
        &mut state,
        &primary,
        "redo",
        serde_json::json!({"filePath":first}),
    );
    assert_eq!(load_editor_state(&first).unwrap().doc.children.len(), 1);
}

#[test]
fn failed_primary_save_rolls_back_edits_undo_redo_and_clipboard() {
    let files = Files::new();
    let good = files.create("good");
    let blocked = files.0.join("blocked.op");
    std::fs::create_dir(&blocked).unwrap();
    let mut state = load_editor_state(&good).unwrap();
    call(&mut state, &good, "insert_node", insert_args());
    call(&mut state, &good, "insert_node", insert_args());
    call(&mut state, &good, "undo", serde_json::json!({}));
    call(
        &mut state,
        &good,
        "set_selection",
        serde_json::json!({"node_id":"n1"}),
    );
    call(&mut state, &good, "copy_selected", serde_json::json!({}));
    let before = state.doc.clone();
    let past = state.history.past.clone();
    let future = state.history.future.clone();
    let clipboard = state.clipboard.clone();
    let selection = state.selection.clone();
    for tool in ["undo", "redo", "cut_selected"] {
        let rejected = response(&mut state, &blocked, tool, serde_json::json!({}));
        assert_eq!(rejected["result"]["isError"], true, "{tool}: {rejected}");
        assert_eq!(state.doc, before, "{tool}");
        assert_eq!(state.history.past, past, "{tool}");
        assert_eq!(state.history.future, future, "{tool}");
        assert_eq!(state.clipboard, clipboard, "{tool}");
        assert_eq!(state.selection, selection, "{tool}");
    }
    assert_eq!(load_editor_state(&good).unwrap().doc, before);
    // A later successful write must not persist the failed cut as a prefix.
    call(
        &mut state,
        &good,
        "update_node",
        serde_json::json!({"node_id":"n1","x":"77"}),
    );
    let saved = load_editor_state(&good).unwrap();
    assert_eq!(saved.doc.children.len(), 1);
    assert_eq!(saved.doc.children[0].base().x, Some(77.0));
}
