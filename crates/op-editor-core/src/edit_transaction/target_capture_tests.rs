//! Private proposed tests; this patch has not been compiled or executed.
use super::{CaptureKind, CommandEditCapture};
use crate::{EditorCommand as C, EditorState, NodeId, PenNodeExt};

fn document(count: usize) -> jian_ops_schema::PenDocument {
    let mut root: jian_ops_schema::node::PenNode =
        serde_json::from_str(r#"{"type":"frame","id":"root","children":[]}"#).unwrap();
    *root.children_mut().unwrap()=(0..count).map(|index|serde_json::from_value(serde_json::json!({"type":"rectangle","id":format!("leaf-{index}"),"x":11.899999618530273,"width":7,"height":9})).unwrap()).collect();
    let mut doc = jian_ops_schema::load_str(r#"{"version":"1","children":[]}"#)
        .unwrap()
        .value;
    doc.children.push(root);
    doc
}
fn state(count: usize) -> EditorState {
    EditorState::from_document(document(count))
}
fn bytes(state: &EditorState) -> Vec<u8> {
    serde_json::to_vec(&state.doc).unwrap()
}
fn update(id: &str, x: i32) -> C {
    C::UpdateNode {
        node_id: NodeId::new(id),
        x: Some(x),
        y: None,
        width: None,
        height: None,
        name: None,
        fill_hex: None,
        page_id: None,
    }
}
fn apply(state: &mut EditorState, command: C) -> CommandEditCapture {
    let mut capture = state.begin_command_edit_without_document_repairs(&command);
    assert!(state.apply_command_edit_then(command, &mut capture, |_| {}));
    capture
}
fn assert_one_shallow_target(capture: &CommandEditCapture) {
    let CaptureKind::Scoped(captured) = &capture.kind else {
        panic!("expected target capture");
    };
    let pending = captured.pending.as_ref().unwrap();
    assert!(pending.is_targeted());
    assert_eq!(
        pending.target_checkpoint_counts(),
        (1, 0),
        "one header and zero descendant nodes owned"
    );
}

#[test]
fn target_container_metadata_keeps_children_and_matches_complete_capture() {
    let mut target = state(512);
    let mut reference = target.clone();
    let before = bytes(&target);
    let children = target.doc.children[0].children().unwrap().as_ptr();
    let command = update("root", 17);
    let mut capture = target.begin_command_edit_without_document_repairs(&command);
    assert_one_shallow_target(&capture);
    assert!(target.apply_command_edit_then(command.clone(), &mut capture, |_| {}));
    assert!(reference.apply_undoable(command));
    assert_eq!(bytes(&target), bytes(&reference));
    assert_eq!(
        target.doc.children[0].children().unwrap().as_ptr(),
        children
    );
    target.abort_command_edit(capture).unwrap();
    assert_eq!(bytes(&target), before);
    assert_eq!(
        target.doc.children[0].children().unwrap().as_ptr(),
        children
    );
}

#[test]
fn target_rich_header_patch_and_copy_override_move_existing_child_vectors() {
    let mut target = state(512);
    let mut reference = target.clone();
    let children = target.doc.children[0].children().unwrap().as_ptr();
    let command = C::PatchNodeData {
        node_id: NodeId::new("root"),
        page_id: None,
        patch_json: r#"{"x":11.899999618530273,"y":-0.0,"name":"changed"}"#.into(),
    };
    let capture = apply(&mut target, command.clone());
    assert!(reference.apply_undoable(command));
    assert_eq!(bytes(&target), bytes(&reference));
    assert_eq!(
        target.doc.children[0].children().unwrap().as_ptr(),
        children
    );
    drop(capture);
    let node = &mut target.doc.children[0];
    crate::node_json::map_header_json_in_place(node, |value| {
        value["name"] = serde_json::json!("another");
    })
    .unwrap();
    assert_eq!(node.children().unwrap().as_ptr(), children);
}

#[test]
fn target_capture_rejects_other_targets_and_unversioned_header_writes() {
    let mut state = state(512);
    let first = update("leaf-0", 3);
    let mut capture = state.begin_command_edit_without_document_repairs(&first);
    let before = bytes(&state);
    assert!(!state.apply_command_edit_then(update("leaf-1", 3), &mut capture, |_| {}));
    assert_eq!(bytes(&state), before);
    state.doc.children[0].children_mut().unwrap()[0]
        .base_mut()
        .name = Some("unversioned".into());
    let current = bytes(&state);
    assert!(!state.apply_command_edit_then(first, &mut capture, |_| {}));
    assert_eq!(bytes(&state), current);
}

#[test]
fn target_metadata_batch_and_header_only_command_keep_complete_rollback() {
    let mut target = state(512);
    let mut reference = target.clone();
    let before = bytes(&target);
    let command = C::Batch {
        commands: vec![
            update("root", 3),
            update("leaf-511", 7),
            C::CreateVariable {
                name: "own".into(),
                kind: "number".into(),
                default_value: "-0.0".into(),
            },
        ],
    };
    let capture = apply(&mut target, command.clone());
    assert!(reference.apply_undoable(command));
    assert_eq!(bytes(&target), bytes(&reference));
    target.abort_command_edit(capture).unwrap();
    assert_eq!(bytes(&target), before);
    let command = C::CreateVariable {
        name: "only-header".into(),
        kind: "number".into(),
        default_value: "-0.0".into(),
    };
    let capture = target.begin_command_edit_without_document_repairs(&command);
    let CaptureKind::Scoped(captured) = capture.kind else {
        panic!()
    };
    assert_eq!(captured.pending.unwrap().target_checkpoint_counts(), (0, 0));
}

#[test]
fn target_undo_redo_uses_reverse_payload_and_external_abort_is_exact() {
    let mut state = state(512);
    let original = bytes(&state);
    drop(apply(&mut state, update("root", 7)));
    let after = bytes(&state);
    let history = state.history.clone();
    let mut undo = state.begin_command_edit_without_document_repairs(&C::Undo);
    assert!(matches!(undo.kind, CaptureKind::History(_)));
    assert!(state.apply_command_edit_then(C::Undo, &mut undo, |_| {}));
    assert_eq!(bytes(&state), original);
    state.abort_command_edit(undo).unwrap();
    assert_eq!(bytes(&state), after);
    assert_eq!(state.history.past, history.past);
    assert_eq!(state.history.future, history.future);
    drop(apply(&mut state, C::Undo));
    assert_eq!(bytes(&state), original);
    drop(apply(&mut state, C::Redo));
    assert_eq!(bytes(&state), after);
}

#[test]
fn target_mode_falls_back_for_dynamic_insert_batches_and_explicit_children() {
    let state = state(512);
    for command in [
        C::PatchNodeData {
            node_id: NodeId::new("root"),
            page_id: None,
            patch_json: r#"{"children":[]}"#.into(),
        },
        C::Batch {
            commands: vec![C::InsertNode {
                kind: "rect".into(),
                name: "new".into(),
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                fill_hex: None,
                target_parent: NodeId::new("root"),
                page_id: None,
            }],
        },
    ] {
        let capture = state.begin_command_edit_without_document_repairs(&command);
        let CaptureKind::Scoped(captured) = capture.kind else {
            panic!()
        };
        assert!(!captured.pending.unwrap().is_targeted());
    }
}

#[test]
#[ignore = "One strict1GB private cached-binary probe after this patch is explicitly applied"]
fn target_capture_largest_bridge_page_leaf_moves_contexts_and_rollback() {
    let mut doc = document(109727);
    let large_children = std::mem::take(&mut doc.children);
    let mut small: jian_ops_schema::page::PenPage = serde_json::from_str(
        r#"{"id":"small","name":"Small","children":[{"type":"rectangle","id":"human","x":-0.0}]}"#,
    )
    .unwrap();
    let mut large: jian_ops_schema::page::PenPage =
        serde_json::from_str(r#"{"id":"large","name":"Large","children":[]}"#).unwrap();
    large.children = large_children;
    // Keep a nonempty small page so the original human view is independent
    // from the large page targeted by this simulated writer context.
    small.background_color = Some("#ffffff".into());
    doc.pages = Some(vec![small, large]);
    let mut state = EditorState::from_document(doc);
    state.set_single_selection(NodeId::new("human"));
    let human_selection = state.selection.clone();
    let before = bytes(&state);
    let children = state.doc.pages.as_ref().unwrap()[1].children[0]
        .children()
        .unwrap()
        .as_ptr();
    // First exercise a huge container header: neither capture nor the typed
    // edit owns/copies any of its109727 descendants.
    let mut command = update("root", 17);
    if let C::UpdateNode { page_id, .. } = &mut command {
        *page_id = Some("large".into());
    }
    let mut capture = state
        .try_begin_command_edit_without_document_repairs(&command)
        .unwrap();
    assert_one_shallow_target(&capture);
    assert!(state.apply_command_edit_then(command, &mut capture, |_| {}));
    state.abort_command_edit(capture).unwrap();
    assert_eq!(bytes(&state), before);
    // Core-only context interleaving control. Actual transport/session and
    // process-memory evidence must come from the existing real service harness.
    state.ui.active_page_index = 1;
    state.set_single_selection(NodeId::new("leaf-109726"));
    let a_selection = state.selection.clone();
    let command = C::PatchNodeData {
        node_id: NodeId::new("leaf-109726"),
        page_id: Some("large".into()),
        patch_json: r#"{"x":17.125,"y":-0.0}"#.into(),
    };
    let mut a = state
        .try_begin_command_edit_without_document_repairs(&command)
        .unwrap();
    assert_one_shallow_target(&a);
    assert!(state.apply_command_edit_then(command, &mut a, |_| {}));
    state.ui.active_page_index = 0;
    state.selection = human_selection.clone();
    let command = update("human", 19);
    let mut b = state
        .try_begin_command_edit_without_document_repairs(&command)
        .unwrap();
    assert_one_shallow_target(&b);
    assert!(state.apply_command_edit_then(command, &mut b, |_| {}));
    // Save failure rolls back each request in reverse under its own context.
    state.abort_command_edit(b).unwrap();
    assert_eq!(state.ui.active_page_index, 0);
    assert_eq!(state.selection, human_selection);
    state.ui.active_page_index = 1;
    state.selection = a_selection.clone();
    state.abort_command_edit(a).unwrap();
    assert_eq!(state.ui.active_page_index, 1);
    assert_eq!(state.selection, a_selection);
    state.ui.active_page_index = 0;
    state.selection = human_selection;
    assert_eq!(bytes(&state), before);
    assert!(state.history.past.is_empty());
    assert_eq!(
        state.doc.pages.as_ref().unwrap()[1].children[0]
            .children()
            .unwrap()
            .as_ptr(),
        children
    );
}
