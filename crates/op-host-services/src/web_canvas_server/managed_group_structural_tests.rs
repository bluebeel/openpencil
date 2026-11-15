//! Consolidated host controls for the bounded structural capture path.
use super::*;

fn calls() -> Vec<String> {
    [
        ("copy_node",json!({"node_id":"n0","parent":"","pageId":"p0"})),
        ("insert_node",json!({"kind":"frame","name":"Inserted","x":"1","y":"2","width":"20","height":"30","parent":"","pageId":"p1"})),
        ("delete_node",json!({"node_id":"human","pageId":"p0"})),
    ].into_iter().enumerate().map(|(index,(name,mut arguments))| {
        arguments["agentId"]=json!(format!("structural{index}"));
        json!({"jsonrpc":"2.0","id":index,"method":"tools/call","params":{"name":name,"arguments":arguments}}).to_string()
    }).collect()
}

#[test]
fn copy_insert_and_delete_share_one_save_with_separate_undo_steps() {
    let files = Files::new();
    let state = files.state(false);
    let original = serde_json::to_vec(&state.lock().unwrap().editor.doc).unwrap();
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    let replies = queued(&ctx(&state, &authority, &hub), &calls());
    assert!(replies
        .iter()
        .all(|reply| reply.get("error").is_none() && reply["result"]["isError"] != true));
    let metrics = authority.managed_groups.metrics.snapshot();
    assert_eq!(metrics["durableSaves"], 1);
    assert_eq!(metrics["savedBatchSizes"]["3"], 1);
    let saved: Value = serde_json::from_slice(&std::fs::read(files.path()).unwrap()).unwrap();
    assert_eq!(
        saved["pages"][0]["children"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["id"],
        "n4"
    );
    let mut state = state.lock().unwrap();
    assert!(
        !state.editor.selection.anchor.is_real(),
        "committed deletion prunes the live human selection"
    );
    assert_eq!(state.editor.history.past.len(), 3);
    for _ in 0..3 {
        assert!(state.editor.undo());
    }
    assert_eq!(serde_json::to_vec(&state.editor.doc).unwrap(), original);
    for _ in 0..3 {
        assert!(state.editor.redo());
    }
    assert_eq!(
        serde_json::to_value(&state.editor.doc).unwrap()["pages"],
        saved["pages"]
    );
}

#[test]
fn structural_group_failed_save_restores_the_live_human_selection_and_document() {
    let files = Files::new();
    let state = files.state(true);
    let original = serde_json::to_vec(&state.lock().unwrap().editor.doc).unwrap();
    let original_selection = state.lock().unwrap().editor.selection.clone();
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    let replies = queued(&ctx(&state, &authority, &hub), &calls());
    assert!(replies
        .iter()
        .all(|reply| reply.get("error").is_some() || reply["result"]["isError"] == true));
    let state = state.lock().unwrap();
    assert_eq!(serde_json::to_vec(&state.editor.doc).unwrap(), original);
    assert_eq!(state.editor.selection, original_selection);
    assert_eq!(state.editor.history.past.len(), 0);
    assert_eq!(state.version, 0);
    assert!(!state.recovery.required());
    assert_eq!(
        std::fs::read(files.path().join("keep")).unwrap(),
        b"prior authority"
    );
}

#[test]
fn target_splices_preserve_absent_and_empty_child_slots_through_rollback_and_history() {
    for children in [None, Some(Vec::<Value>::new())] {
        let mut parent = json!({"type":"group","id":"parent"});
        if let Some(children) = children {
            parent["children"] = json!(children);
        }
        let doc = serde_json::from_value(json!({"version":"1.0.0","children":[parent]})).unwrap();
        let mut editor = EditorState::from_document(doc);
        let before = serde_json::to_vec(&editor.doc).unwrap();
        let command = C::InsertNode {
            kind: "rectangle".into(),
            name: "child".into(),
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            fill_hex: None,
            target_parent: NodeId::new("parent"),
            page_id: None,
        };
        let mut capture = editor
            .try_begin_command_edit_without_document_repairs(&command)
            .expect("bounded authored insertion");
        assert!(capture.is_target_only());
        assert!(editor.apply_command_edit_then(command.clone(), &mut capture, |_| {}));
        editor.abort_command_edit(capture).unwrap();
        assert_eq!(serde_json::to_vec(&editor.doc).unwrap(), before);
        let mut capture = editor
            .try_begin_command_edit_without_document_repairs(&command)
            .unwrap();
        assert!(editor.apply_command_edit_then(command, &mut capture, |_| {}));
        let after = serde_json::to_vec(&editor.doc).unwrap();
        assert!(editor.undo());
        assert_eq!(serde_json::to_vec(&editor.doc).unwrap(), before);
        assert!(editor.redo());
        assert_eq!(serde_json::to_vec(&editor.doc).unwrap(), after);
    }
}

#[test]
fn ambiguous_parent_copy_and_large_subtree_keep_the_existing_fallback() {
    let document = serde_json::from_value(json!({"version":"1.0.0","children":[
        {"type":"group","id":"outer","children":[{"type":"group","id":"parent"}]},
        {"type":"group","id":"parent"},{"type":"rectangle","id":"source","width":10,"height":10}
    ]}))
    .unwrap();
    let editor = EditorState::from_document(document);
    let command = C::CopyNode {
        node_id: NodeId::new("source"),
        target_parent: NodeId::new("parent"),
        overrides_json: None,
        page_id: None,
    };
    assert!(editor
        .try_begin_command_edit_without_document_repairs(&command)
        .is_none());
    let mut document = serde_json::to_value(&editor.doc).unwrap();
    document["children"][0]["children"] = json!((0..4097)
        .map(|i| json!({"type":"rectangle","id":format!("large{i}")}))
        .collect::<Vec<_>>());
    let editor = EditorState::from_document(serde_json::from_value(document).unwrap());
    let command = C::DeleteNode {
        node_id: NodeId::new("outer"),
        page_id: None,
    };
    assert!(editor
        .try_begin_command_edit_without_document_repairs(&command)
        .is_none());
}
