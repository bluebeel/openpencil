use super::{apply, bytes, state};
use crate::{EditorCommand as C, EditorState, NodeId, PenNodeExt};

fn paired(scoped: &mut EditorState, full: &mut EditorState, command: C) {
    assert!(
        full.apply_undoable(command.clone()),
        "legacy command {command:?}"
    );
    apply(scoped, command);
    assert_eq!(bytes(scoped), bytes(full));
}
fn retained_nodes(state: &EditorState) -> usize {
    fn count(node: &crate::SharedNode) -> usize {
        1 + node
            .children()
            .map_or(0, |nodes| nodes.iter().map(|node| count(node)).sum())
    }
    state
        .history
        .past
        .back()
        .unwrap()
        .scoped_snapshot()
        .unwrap()
        .retained_roots()
        .map(|node| count(node))
        .sum()
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
fn move_to(id: &str, parent: Option<&str>, index: usize) -> C {
    C::MoveNode {
        node_id: NodeId::new(id),
        target_parent: parent.map_or(NodeId::NONE, NodeId::new),
        index: Some(index),
        page_id: None,
    }
}

#[test]
fn nested_structure_keeps_removed_nodes_not_the_large_parent() {
    let mut scoped = state();
    let template = serde_json::from_str::<jian_ops_schema::node::PenNode>(
        r#"{"type":"rectangle","id":"template","width":12,"height":13}"#,
    )
    .unwrap();
    let empty = crate::walkers::find_node_mut(
        &mut scoped.doc.pages.as_mut().unwrap()[0].children,
        &NodeId::new("empty"),
    )
    .unwrap();
    *empty.children_mut().unwrap() = (0..512)
        .map(|index| {
            let mut node = template.clone();
            node.base_mut().id = format!("leaf-{index}");
            node
        })
        .collect();
    let mut full = scoped.clone();
    let before = bytes(&scoped);
    paired(
        &mut scoped,
        &mut full,
        C::InsertNode {
            kind: "rect".into(),
            name: "new".into(),
            x: 1,
            y: 2,
            width: 3,
            height: 4,
            fill_hex: None,
            target_parent: NodeId::new("empty"),
            page_id: None,
        },
    );
    assert_eq!(
        retained_nodes(&scoped),
        0,
        "no unchanged parent/children retained for insertion"
    );
    let inserted = crate::walkers::find_node(scoped.active_children(), &NodeId::new("empty"))
        .unwrap()
        .children()
        .unwrap()
        .last()
        .unwrap()
        .id_str()
        .to_owned();
    paired(
        &mut scoped,
        &mut full,
        move_to("leaf-0", Some("empty"), 200),
    );
    assert_eq!(retained_nodes(&scoped), 0);
    paired(
        &mut scoped,
        &mut full,
        C::DeleteNode {
            node_id: NodeId::new(&inserted),
            page_id: None,
        },
    );
    assert_eq!(retained_nodes(&scoped), 1);
    paired(
        &mut scoped,
        &mut full,
        C::SetSelectionSet {
            node_ids: vec![NodeId::new("leaf-1"), NodeId::new("leaf-2")],
        },
    );
    paired(&mut scoped, &mut full, C::GroupSelected);
    assert_eq!(
        retained_nodes(&scoped),
        2,
        "only the two removed sibling records belong to old history"
    );
    let after = bytes(&scoped);
    while full.history.can_undo() {
        assert!(full.undo());
        assert!(scoped.undo());
        assert_eq!(bytes(&scoped), bytes(&full));
    }
    assert_eq!(bytes(&scoped), before);
    while full.history.can_redo() {
        assert!(full.redo());
        assert!(scoped.redo());
        assert_eq!(bytes(&scoped), bytes(&full));
    }
    assert_eq!(bytes(&scoped), after);
}

fn nested() -> EditorState {
    EditorState::from_document(
        serde_json::from_str(
            r#"{"version":"1","children":[
      {"type":"frame","id":"a","children":[
        {"type":"frame","id":"p","children":[
          {"type":"frame","id":"q","children":[
            {"type":"rectangle","id":"x","x":-0.0},
            {"type":"rectangle","id":"y"},{"type":"rectangle","id":"z"}]},
          {"type":"rectangle","id":"s"}]},
        {"type":"rectangle","id":"aside"}]},
      {"type":"frame","id":"b"}] }"#,
        )
        .unwrap(),
    )
}

#[test]
fn nested_orders_rebase_every_ancestor_and_reverse_after_external_abort() {
    let mut scoped = nested();
    let mut full = scoped.clone();
    let before = bytes(&scoped);
    let command = C::Batch {
        commands: vec![
            update("x", 17),
            update("p", 9),
            move_to("x", Some("q"), 2),
            move_to("q", Some("p"), 1),
            move_to("p", Some("a"), 1),
            move_to("a", None, 1),
        ],
    };
    let mut capture = scoped.begin_command_edit(&command);
    assert!(capture.is_scoped());
    assert!(scoped.apply_command_edit_then(command.clone(), &mut capture, |_| {}));
    assert!(full.apply_undoable(command.clone()));
    assert_eq!(bytes(&scoped), bytes(&full));
    assert_eq!(retained_nodes(&scoped), 2, "changed shallow headers only");
    let after = bytes(&scoped);
    assert!(scoped.undo());
    assert!(full.undo());
    assert_eq!(bytes(&scoped), before);
    assert_eq!(bytes(&scoped), bytes(&full));
    assert!(scoped.redo());
    assert!(full.redo());
    assert_eq!(bytes(&scoped), after);
    assert_eq!(bytes(&scoped), bytes(&full));
    // The original pending commit may be rejected after an undo/redo view;
    // its retained one-sided snapshot still restores the exact pre-command.
    scoped.abort_command_edit(capture).unwrap();
    assert_eq!(bytes(&scoped), before);
    assert!(scoped.history.past.is_empty());
}

#[test]
fn duplicate_nested_children_keep_local_fallback_through_ancestor_order() {
    let mut scoped = EditorState::from_document(
        serde_json::from_str(
            r#"{"version":"1","children":[
      {"type":"frame","id":"a","children":[{"type":"frame","id":"p","children":[
        {"type":"rectangle","id":"dup","x":-0.0},
        {"type":"rectangle","id":"dup","x":11.899999618530273}]},
        {"type":"text","id":"untouched","content":"same"}]},
      {"type":"frame","id":"b"}] }"#,
        )
        .unwrap(),
    );
    let mut full = scoped.clone();
    let before = bytes(&scoped);
    fn maintain(state: &mut EditorState) {
        let parent =
            crate::walkers::find_node_mut(state.active_children_mut(), &NodeId::new("p")).unwrap();
        parent.base_mut().name = Some("maintained".into());
        let children = parent.children_mut().unwrap();
        children.insert(0, children[1].clone());
    }
    let command = move_to("a", None, 1);
    let mut capture = scoped.begin_command_edit(&command);
    assert!(full.apply_undoable_then(command.clone(), maintain));
    assert!(scoped.apply_command_edit_then(command, &mut capture, maintain));
    assert_eq!(bytes(&scoped), bytes(&full));
    assert_eq!(
        retained_nodes(&scoped),
        3,
        "two old children plus shallow parent, never ancestor forest"
    );
    let after = bytes(&scoped);
    assert!(scoped.undo());
    assert!(full.undo());
    assert_eq!(bytes(&scoped), before);
    assert_eq!(bytes(&scoped), bytes(&full));
    assert!(scoped.redo());
    assert!(full.redo());
    assert_eq!(bytes(&scoped), after);
    assert_eq!(bytes(&scoped), bytes(&full));
}

#[test]
fn nested_child_presence_changes_still_match_full_history() {
    let mut scoped = state();
    let mut full = scoped.clone();
    let before = bytes(&scoped);
    for patch_json in [
        r#"{"children":null}"#,
        r#"{"children":[{"type":"rectangle","id":"inside","x":-0.0}]}"#,
        r#"{"children":[]}"#,
    ] {
        paired(
            &mut scoped,
            &mut full,
            C::PatchNodeData {
                node_id: NodeId::new("empty"),
                page_id: None,
                patch_json: patch_json.into(),
            },
        );
    }
    let after = bytes(&scoped);
    while full.history.can_undo() {
        assert!(full.undo());
        assert!(scoped.undo());
        assert_eq!(bytes(&scoped), bytes(&full));
    }
    assert_eq!(bytes(&scoped), before);
    while full.history.can_redo() {
        assert!(full.redo());
        assert!(scoped.redo());
        assert_eq!(bytes(&scoped), bytes(&full));
    }
    assert_eq!(bytes(&scoped), after);
}

#[test]
fn full_replacement_restore_keeps_scoped_undo_redo_and_clone_isolation() {
    let mut scoped = state();
    let mut full = scoped.clone();
    let original = bytes(&scoped);
    for x in [1, 2] {
        paired(&mut scoped, &mut full, update("a", x));
    }
    let mut clone = scoped.clone();
    let mut replacement = scoped.doc.clone();
    replacement.name = Some("replacement".into());
    crate::walkers::find_node_mut(
        &mut replacement.pages.as_mut().unwrap()[0].children,
        &NodeId::new("a"),
    )
    .unwrap()
    .base_mut()
    .x = Some(999.0);
    scoped.replace_document_with_undo(replacement.clone());
    full.replace_document_with_undo(replacement);
    paired(&mut scoped, &mut full, update("a", 7));
    let after = bytes(&scoped);
    assert!(scoped.undo());
    assert!(full.undo());
    assert_eq!(bytes(&scoped), bytes(&full));
    // Full Undo is also an external-commit transaction; aborting it restores
    // the original history segment without rebasing its shared clone's arcs.
    let before_full_undo = bytes(&scoped);
    let mut capture = scoped.begin_command_edit(&C::Undo);
    assert!(!capture.is_scoped());
    assert!(scoped.apply_command_edit_then(C::Undo, &mut capture, |_| {}));
    scoped.abort_command_edit(capture).unwrap();
    assert_eq!(bytes(&scoped), before_full_undo);
    for _ in 0..3 {
        assert!(scoped.undo());
        assert!(full.undo());
        assert_eq!(bytes(&scoped), bytes(&full));
    }
    assert_eq!(bytes(&scoped), original);
    // Two original scoped entries remain usable in the independent generation.
    assert!(clone.undo());
    assert!(clone.undo());
    assert_eq!(bytes(&clone), original);
    for _ in 0..4 {
        assert!(scoped.redo());
        assert!(full.redo());
        assert_eq!(bytes(&scoped), bytes(&full));
    }
    assert_eq!(bytes(&scoped), after);
}
