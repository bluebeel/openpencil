use super::*;
use op_editor_core::{NodeId, PenNodeExt};

fn state() -> EditorState {
    let mut doc = jian_ops_schema::load_str(
        r#"{"version":"1","children":[{"type":"frame","id":"root","children":[]}]}"#,
    )
    .unwrap()
    .value;
    *doc.children[0].children_mut().unwrap() = (0..512).map(|index| {
        serde_json::from_value(serde_json::json!({"type":"rectangle","id":format!("leaf-{index}"),"x":null,"width":7,"height":9})).unwrap()
    }).collect();
    EditorState::from_document(doc)
}
fn bytes(state: &EditorState) -> Vec<u8> {
    serde_json::to_vec(&state.doc).unwrap()
}
fn movement() -> EditorCommand {
    EditorCommand::PatchNodeData {
        node_id: NodeId::new("leaf-511"),
        page_id: None,
        patch_json: r#"{"x":11.899999618530273,"y":-0.0}"#.into(),
    }
}

#[test]
fn ordinary_move_prepares_only_target_even_when_generation_repairs_enabled() {
    let mut state = state();
    let before = bytes(&state);
    let children = state.doc.children[0].children().unwrap().as_ptr();
    let command = movement();
    let mut capture = begin_command_capture(&state, &command, true);
    assert!(capture.capture.is_target_only());
    assert!(apply_command_with_command_capture(
        &mut state,
        command.clone(),
        &mut capture
    ));
    assert!(capture.capture.is_target_only());
    assert_eq!(state.doc.children[0].children().unwrap().as_ptr(), children);
    assert!(
        !apply_command_with_command_capture(&mut state, command, &mut capture),
        "a prepared command is single-use"
    );
    capture.abort(&mut state).unwrap();
    assert_eq!(bytes(&state), before);
    assert!(state.history.past.is_empty());
}

#[test]
fn actual_generation_repair_keeps_broad_capture_and_legacy_result() {
    let mut state = state();
    let mut reference = state.clone();
    let node=serde_json::from_str(r#"{"type":"frame","id":"new","width":375,"height":812,"children":[{"type":"text","id":"label","content":"new"}]}"#).unwrap();
    let command = EditorCommand::InsertSubtree {
        nodes: vec![node],
        parent_id: NodeId::NONE,
        page_id: None,
    };
    let mut capture = begin_command_capture(&state, &command, true);
    assert!(!capture.capture.is_target_only());
    let plan = RepairPlan::capture(&reference, &command, true);
    assert!(!plan.is_empty());
    let revision = reference.document_revision();
    assert!(reference.apply_undoable_then(command.clone(), |state| {
        plan.apply(state, revision);
    }));
    assert!(apply_command_with_command_capture(
        &mut state,
        command,
        &mut capture
    ));
    assert_eq!(bytes(&state), bytes(&reference));
}

#[test]
fn refused_move_keeps_document_and_redo_without_page_checkpoint() {
    let mut state = state();
    let command = movement();
    assert!(apply_command_with_undo(&mut state, command, false));
    assert!(state.undo());
    let before = bytes(&state);
    let history = state.history.clone();
    let command = EditorCommand::PatchNodeData {
        node_id: NodeId::new("leaf-511"),
        page_id: Some("missing".into()),
        patch_json: r#"{"x":19.25}"#.into(),
    };
    let mut capture = begin_command_capture(&state, &command, true);
    assert!(capture.capture.is_target_only());
    assert!(!apply_command_with_command_capture(
        &mut state,
        command,
        &mut capture
    ));
    assert_eq!(bytes(&state), before);
    assert_eq!(state.history.past, history.past);
    assert_eq!(state.history.future, history.future);
}

#[test]
fn stdio_target_move_save_failure_restores_exact_document_and_history() {
    let mut state = state();
    let before = bytes(&state);
    let revision = state.document_revision();
    let path = std::env::temp_dir().join(format!(
        "op-target-capture-save-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    let line=serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"update_node","arguments":{"nodeId":"leaf-511","data":{"x":11.899999618530273,"y":-0.0}}}}).to_string();
    let response = super::super::stdio_dispatch::process_message_with_auto_finalize(
        &mut state,
        &path,
        &line,
        None,
        super::super::tool_profile::McpAccessProfile::UNRESTRICTED,
    )
    .unwrap()
    .unwrap();
    assert!(
        !super::super::mcp_response_succeeded(&response),
        "{response}"
    );
    assert_eq!(bytes(&state), before);
    assert_eq!(state.document_revision(), revision);
    assert!(state.history.past.is_empty());
    std::fs::remove_dir(&path).unwrap();
}
