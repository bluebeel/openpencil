use crate::{EditorCommand as C, EditorState, HistoryEntry, NodeId, PenNodeExt};
fn state() -> EditorState {
    EditorState::from_document(serde_json::from_str(r#"{"version":"1","pages":[{"id":"page-a","name":"A","children":[{"type":"frame","id":"root-a","children":[{"type":"rectangle","id":"a","x":-0.0,"width":20,"height":30},{"type":"frame","id":"empty","children":[]},{"type":"ref","id":"ref","ref":"root-a"}]}]},{"id":"page-b","name":"B","children":[{"type":"frame","id":"root-b","children":[{"type":"text","id":"b","content":"before","events":{"future":{"v":-0.0}}}]}]},{"id":"untouched","name":"C","children":[{"type":"rectangle","id":"c","x":11.899999618530273}]}]}"#).unwrap())
}
fn bytes(state: &EditorState) -> Vec<u8> {
    serde_json::to_vec(&state.doc).unwrap()
}
fn update(id: &str, x: i32, page: Option<&str>) -> C {
    C::UpdateNode {
        node_id: NodeId::new(id),
        x: Some(x),
        y: None,
        width: None,
        height: None,
        name: None,
        fill_hex: None,
        page_id: page.map(str::to_owned),
    }
}
fn apply(state: &mut EditorState, command: C) {
    let mut capture = state.begin_command_edit(&command);
    assert!(capture.is_scoped(), "{command:?}");
    assert!(state.apply_command_edit_then(command, &mut capture, |_| {}));
}
#[test]
fn scoped_cross_page_patch_and_maintenance_match_full_undo_redo() {
    let mut scoped = state();
    let before = bytes(&scoped);
    let mut full = scoped.clone();
    let command = C::PatchNodeData {
        node_id: NodeId::new("b"),
        page_id: Some("page-b".into()),
        patch_json: r#"{"x":11.899999618530273,"y":-0.0,"content":"after"}"#.into(),
    };
    fn maintain(state: &mut EditorState) {
        state.doc.pages.as_mut().unwrap()[0].children[0]
            .base_mut()
            .name = Some("maintained".into());
    }
    assert!(full.apply_undoable_then(command.clone(), maintain));
    let mut capture = scoped.begin_command_edit(&command);
    assert!(capture.is_scoped());
    assert!(scoped.apply_command_edit_then(command, &mut capture, maintain));
    let after = bytes(&scoped);
    assert_eq!(after, bytes(&full));
    let snapshot = scoped
        .history
        .past
        .back()
        .unwrap()
        .scoped_snapshot()
        .unwrap();
    assert_eq!(
        snapshot.retained_roots().count(),
        2,
        "unchanged pages/descendants must not be retained"
    );
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
fn scoped_node_crud_matches_full_and_reverses_structure_exactly() {
    let mut scoped = state();
    let mut full = scoped.clone();
    let before = bytes(&scoped);
    let commands = vec![
        update("a", 17, None),
        C::InsertNode {
            kind: "rect".into(),
            name: "new".into(),
            x: 2,
            y: 3,
            width: 4,
            height: 5,
            fill_hex: None,
            target_parent: NodeId::new("empty"),
            page_id: None,
        },
        C::CopyNode {
            node_id: NodeId::new("a"),
            target_parent: NodeId::new("root-b"),
            overrides_json: None,
            page_id: Some("page-a".into()),
        },
    ];
    // Invalid cross-page parent preserves both histories, matching legacy.
    for command in commands {
        let expected = full.apply_undoable(command.clone());
        let mut capture = scoped.begin_command_edit(&command);
        assert!(capture.is_scoped());
        assert_eq!(
            scoped.apply_command_edit_then(command, &mut capture, |_| {}),
            expected
        );
        assert_eq!(bytes(&scoped), bytes(&full));
    }
    for command in [
        C::CopyNode {
            node_id: NodeId::new("a"),
            target_parent: NodeId::new("empty"),
            overrides_json: None,
            page_id: None,
        },
        C::MoveNode {
            node_id: NodeId::new("a"),
            target_parent: NodeId::new("empty"),
            page_id: None,
            index: Some(0),
        },
        C::DeleteNode {
            node_id: NodeId::new("a"),
            page_id: None,
        },
    ] {
        assert!(full.apply_undoable(command.clone()));
        apply(&mut scoped, command);
        assert_eq!(bytes(&scoped), bytes(&full));
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
fn scoped_history_retains_changed_nodes_not_the_whole_temporary_page() {
    let mut state = state();
    let root = state.doc.pages.as_mut().unwrap()[2].children[0].clone();
    state.doc.pages.as_mut().unwrap()[2].children = (0..512)
        .map(|index| {
            let mut node = root.clone();
            node.base_mut().id = format!("other-{index}");
            node
        })
        .collect();
    for x in 1..=8 {
        apply(&mut state, update("a", x, None));
    }
    let retained: usize = state
        .history
        .past
        .iter()
        .map(|entry| {
            entry
                .scoped_snapshot()
                .unwrap()
                .retained_roots()
                .map(|node| {
                    assert!(node.children().is_none());
                    1
                })
                .sum::<usize>()
        })
        .sum();
    assert_eq!(retained, 8);
    assert!(state
        .history
        .past
        .iter()
        .all(|entry| matches!(entry, HistoryEntry::Scoped(_))));
}
#[test]
fn scoped_abort_restores_document_history_auxiliary_and_undo_transaction() {
    let mut state = state();
    apply(&mut state, update("a", 1, None));
    state
        .ui
        .variables
        .fill_refs
        .insert(NodeId::new("a"), "$token".into());
    let before = bytes(&state);
    let history = state.history.clone();
    let revision = state.document_revision();
    let selection = state.selection.clone();
    let command = update("a", 2, None);
    let mut capture = state.begin_command_edit(&command);
    assert!(
        state.apply_command_edit_then(command, &mut capture, |state| {
            state.ui.variables.fill_refs.clear();
        })
    );
    state.abort_command_edit(capture).unwrap();
    assert_eq!(bytes(&state), before);
    assert_eq!(state.history.past, history.past);
    assert_eq!(state.history.future, history.future);
    assert_eq!(state.document_revision(), revision);
    assert_eq!(state.selection, selection);
    assert_eq!(state.ui.variables.fill_refs[&NodeId::new("a")], "$token");
    let mut capture = state.begin_command_edit(&C::Undo);
    assert!(capture.is_scoped());
    assert!(state.apply_command_edit_then(C::Undo, &mut capture, |_| {}));
    state.abort_command_edit(capture).unwrap();
    assert_eq!(bytes(&state), before);
    assert_eq!(state.history.past, history.past);
    assert_eq!(state.history.future, history.future);
}
#[test]
fn scoped_invalid_and_mismatched_commands_leave_redo_and_document_unchanged() {
    let mut state = state();
    apply(&mut state, update("a", 1, None));
    assert!(state.undo());
    let before = bytes(&state);
    let history = state.history.clone();
    let command = C::UpdateNode {
        node_id: NodeId::new("a"),
        x: Some(99),
        y: None,
        width: Some(-1),
        height: None,
        name: None,
        fill_hex: None,
        page_id: None,
    };
    let mut capture = state.begin_command_edit(&command);
    assert!(
        !state.apply_command_edit_then(command, &mut capture, |_| panic!(
            "invalid command must not maintain"
        ))
    );
    assert_eq!(bytes(&state), before);
    assert_eq!(state.history.past, history.past);
    assert_eq!(state.history.future, history.future);
    let mut capture = state.begin_command_edit(&update("a", 2, None));
    assert!(!state.apply_command_edit_then(update("b", 3, Some("page-b")), &mut capture, |_| {}));
    assert_eq!(bytes(&state), before);
    assert_eq!(state.history.future, history.future);
}
#[test]
fn scoped_panic_keeps_prefix_undoable_and_clone_history_isolated() {
    let mut state = state();
    let before = bytes(&state);
    let command = update("a", 1, None);
    let mut capture = state.begin_command_edit(&command);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.apply_command_edit_then(command, &mut capture, |state| {
            state.doc.pages.as_mut().unwrap()[0].children[0]
                .base_mut()
                .name = Some("prefix".into());
            panic!("maintenance");
        })
    }));
    assert!(panic.is_err());
    assert_eq!(state.history.past.len(), 1);
    let clone = state.clone();
    let after = bytes(&state);
    assert!(state.undo());
    assert_eq!(bytes(&state), before);
    assert_eq!(bytes(&clone), after);
}
#[test]
fn scoped_global_commands_and_active_workspace_keep_full_snapshots() {
    let mut state = state();
    let command = C::ReplaceFontFamily {
        from: "before".into(),
        to: "after".into(),
    };
    assert!(!state.begin_command_edit(&command).is_scoped());
    state.editor_ui.workspace.active = true;
    assert!(!state.begin_command_edit(&update("a", 1, None)).is_scoped());
}

#[test]
fn scoped_history_cap_rollback_preserves_past_redo_and_revision() {
    let mut state = state();
    for x in 1..=crate::HISTORY_CAP + 3 {
        apply(&mut state, update("a", x as i32, None));
    }
    assert_eq!(state.history.past.len(), crate::HISTORY_CAP);
    assert!(state.undo());
    let before = bytes(&state);
    let past = state.history.past.clone();
    let future = state.history.future.clone();
    let revision = state.document_revision();
    let command = update("a", 999, None);
    let mut capture = state.begin_command_edit(&command);
    assert!(state.apply_command_edit_then(command, &mut capture, |_| {}));
    state.abort_command_edit(capture).unwrap();
    assert_eq!(bytes(&state), before);
    assert_eq!(state.history.past, past);
    assert_eq!(state.history.future, future);
    assert_eq!(state.document_revision(), revision);
    assert!(state.redo());
    assert!(state.undo());
    assert_eq!(bytes(&state), before);
}

#[test]
fn scoped_literal_page_ids_precede_indices_and_duplicate_nodes_stay_page_local() {
    let mut state = state();
    state.doc.pages.as_mut().unwrap()[0].id = "1".into();
    state.doc.pages.as_mut().unwrap()[1].children[0]
        .children_mut()
        .unwrap()[0]
        .base_mut()
        .id = "a".into();
    let before = bytes(&state);
    let mut full = state.clone();
    state.ui.active_page_index = 1;
    full.ui.active_page_index = 1;
    let command = update("a", 27, Some("1"));
    assert!(full.apply_undoable(command.clone()));
    apply(&mut state, command);
    assert_eq!(bytes(&state), bytes(&full));
    assert!(state.undo());
    assert_eq!(bytes(&state), before);
    let command = update("a", 31, Some("page-b"));
    apply(&mut state, command);
    assert_eq!(
        state.doc.pages.as_ref().unwrap()[0].children[0]
            .children()
            .unwrap()[0]
            .base()
            .x
            .unwrap()
            .to_bits(),
        (-0.0_f64).to_bits()
    );
    assert!(state.undo());
    assert_eq!(bytes(&state), before);
}

#[test]
fn scoped_root_insert_delete_reorder_retains_only_removed_roots() {
    let mut state = state();
    let original = bytes(&state);
    let command = C::InsertNode {
        kind: "rect".into(),
        name: "new-root".into(),
        x: 1,
        y: 2,
        width: 3,
        height: 4,
        fill_hex: None,
        target_parent: NodeId::NONE,
        page_id: None,
    };
    apply(&mut state, command);
    let inserted = state.doc.pages.as_ref().unwrap()[0]
        .children
        .last()
        .unwrap()
        .base()
        .id
        .clone();
    assert_eq!(
        state
            .history
            .past
            .back()
            .unwrap()
            .scoped_snapshot()
            .unwrap()
            .retained_roots()
            .count(),
        0,
        "insertion must not retain unchanged full roots"
    );
    apply(
        &mut state,
        C::MoveNode {
            node_id: NodeId::new(&inserted),
            target_parent: NodeId::NONE,
            page_id: None,
            index: Some(0),
        },
    );
    let reordered = bytes(&state);
    assert_eq!(
        state
            .history
            .past
            .back()
            .unwrap()
            .scoped_snapshot()
            .unwrap()
            .retained_roots()
            .count(),
        0
    );
    apply(
        &mut state,
        C::DeleteNode {
            node_id: NodeId::new(&inserted),
            page_id: None,
        },
    );
    assert_eq!(
        state
            .history
            .past
            .back()
            .unwrap()
            .scoped_snapshot()
            .unwrap()
            .retained_roots()
            .count(),
        1
    );
    assert!(state.undo());
    assert_eq!(bytes(&state), reordered);
    assert!(state.undo());
    assert!(state.undo());
    assert_eq!(bytes(&state), original);
    assert!(state.redo());
    assert!(state.redo());
    assert_eq!(bytes(&state), reordered);
    assert!(state.redo());
}

#[test]
fn scoped_duplicate_root_ids_preserve_indexed_maintenance_swaps() {
    let mut state = state();
    let duplicate = state.doc.pages.as_ref().unwrap()[0].children[0].clone();
    state.doc.pages.as_mut().unwrap()[0]
        .children
        .push(duplicate);
    let before = bytes(&state);
    let command = update("a", 4, None);
    let mut capture = state.begin_command_edit(&command);
    assert!(
        state.apply_command_edit_then(command, &mut capture, |state| state
            .doc
            .pages
            .as_mut()
            .unwrap()[0]
            .children[1]
            .base_mut()
            .name =
            Some("second".into()))
    );
    let after = bytes(&state);
    assert!(state.undo());
    assert_eq!(bytes(&state), before);
    assert!(state.redo());
    assert_eq!(bytes(&state), after);
}

#[test]
fn scoped_batch_created_parent_and_page_cursor_match_full_atomic_history() {
    let mut scoped = state();
    let mut full = scoped.clone();
    let before = bytes(&scoped);
    let parent = serde_json::from_str(r#"{"type":"frame","id":"created-parent"}"#).unwrap();
    let command = C::Batch {
        commands: vec![
            C::InsertAuthoredSubtreePreservingRoots {
                nodes: vec![parent],
                parent_id: NodeId::new("root-a"),
                page_id: None,
            },
            C::InsertNode {
                kind: "rect".into(),
                name: "inside".into(),
                x: 1,
                y: 2,
                width: 3,
                height: 4,
                fill_hex: None,
                target_parent: NodeId::new("created-parent"),
                page_id: None,
            },
            update("created-parent", 7, None),
            C::SetActivePage { index: 1 },
            update("b", 13, None),
        ],
    };
    assert!(full.apply_undoable(command.clone()));
    apply(&mut scoped, command);
    let after = bytes(&scoped);
    assert_eq!(after, bytes(&full));
    assert_eq!(scoped.history.past.len(), 1);
    assert!(scoped.undo());
    assert!(full.undo());
    assert_eq!(bytes(&scoped), before);
    assert_eq!(bytes(&scoped), bytes(&full));
    assert!(scoped.redo());
    assert_eq!(bytes(&scoped), after);
}

#[test]
fn scoped_batch_late_failure_and_nested_rejection_restore_exact_prefix_and_redo() {
    let mut scoped = state();
    apply(&mut scoped, update("a", 1, None));
    assert!(scoped.undo());
    let before = bytes(&scoped);
    let past = scoped.history.past.clone();
    let future = scoped.history.future.clone();
    let revision = scoped.document_revision();
    let bad = C::UpdateNode {
        node_id: NodeId::new("a"),
        x: None,
        y: None,
        width: Some(-1),
        height: None,
        name: None,
        fill_hex: None,
        page_id: None,
    };
    for command in [
        C::Batch {
            commands: vec![update("a", 7, None), bad],
        },
        C::Batch {
            commands: vec![
                update("a", 8, None),
                C::Batch {
                    commands: vec![update("a", 9, None)],
                },
            ],
        },
    ] {
        let mut full = scoped.clone();
        assert!(!full.apply_undoable(command.clone()));
        let mut capture = scoped.begin_command_edit(&command);
        assert!(capture.is_scoped());
        assert!(
            !scoped.apply_command_edit_then(command, &mut capture, |_| panic!(
                "failed Batch maintenance"
            ))
        );
        assert_eq!(bytes(&scoped), before);
        assert_eq!(bytes(&scoped), bytes(&full));
        assert_eq!(scoped.history.past, past);
        assert_eq!(scoped.history.future, future);
        assert_eq!(scoped.document_revision(), revision);
    }
}
#[test]
fn scoped_component_lifecycle_matches_full_registry_and_canonical_state() {
    let mut scoped = state();
    let before = bytes(&scoped);
    let mut full = scoped.clone();
    for command in [
        C::CreateComponent {
            node_id: NodeId::new("root-a"),
            name: "Master".into(),
        },
        C::InstantiateComponent {
            component_id: NodeId::new("root-a"),
        },
        C::RenameComponent {
            component_id: NodeId::new("root-a"),
            name: "Renamed".into(),
        },
        C::DeleteComponent {
            component_id: NodeId::new("root-a"),
        },
    ] {
        let template =
            serde_json::to_vec(&scoped.doc.pages.as_ref().unwrap()[0].children[0]).unwrap();
        assert!(full.apply_undoable(command.clone()));
        apply(&mut scoped, command.clone());
        if matches!(command, C::InstantiateComponent { .. }) {
            assert_eq!(
                serde_json::to_vec(&scoped.doc.pages.as_ref().unwrap()[0].children[0]).unwrap(),
                template,
                "instantiating a borrowed template must not mutate its source"
            );
        }
        assert_eq!(bytes(&scoped), bytes(&full));
        assert_eq!(
            crate::SharedComponents::capture(&scoped.components, None),
            crate::SharedComponents::capture(&full.components, None)
        );
    }
    let after = bytes(&scoped);
    while full.history.can_undo() {
        assert!(full.undo());
        assert!(scoped.undo());
        assert_eq!(bytes(&scoped), bytes(&full));
        assert_eq!(
            crate::SharedComponents::capture(&scoped.components, None),
            crate::SharedComponents::capture(&full.components, None)
        );
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
fn scoped_component_delete_scopes_source_page_and_active_maintenance() {
    let mut scoped = state();
    apply(
        &mut scoped,
        C::CreateComponent {
            node_id: NodeId::new("root-a"),
            name: "Master".into(),
        },
    );
    scoped.ui.active_page_index = 1;
    let before = bytes(&scoped);
    let mut full = scoped.clone();
    fn maintain(state: &mut EditorState) {
        state.doc.pages.as_mut().unwrap()[1].children[0]
            .base_mut()
            .name = Some("maintenance".into());
    }
    let command = C::DeleteComponent {
        component_id: NodeId::new("root-a"),
    };
    assert!(full.apply_undoable_then(command.clone(), maintain));
    let mut capture = scoped.begin_command_edit(&command);
    assert!(capture.is_scoped());
    assert!(scoped.apply_command_edit_then(command, &mut capture, maintain));
    assert_eq!(bytes(&scoped), bytes(&full));
    assert!(scoped.undo());
    assert!(full.undo());
    assert_eq!(bytes(&scoped), before);
    assert_eq!(bytes(&scoped), bytes(&full));
    assert!(scoped
        .components
        .find_by_id(&NodeId::new("root-a"))
        .is_some());
}
