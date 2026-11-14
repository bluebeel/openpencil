use crate::{test_support::sample, DocumentIdAllocator, EditorCommand, IdAllocError, NodeId};

fn nudge(dx: i32, dy: i32) -> EditorCommand {
    EditorCommand::NudgeSelected { dx, dy }
}

fn insert() -> EditorCommand {
    EditorCommand::InsertNode {
        kind: "rect".into(),
        name: "Undoable insertion".into(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        fill_hex: None,
        target_parent: NodeId::NONE,
        page_id: None,
    }
}

#[test]
fn raw_crud_commands_each_have_one_exact_undo_redo_step() {
    let mut state = sample();
    let commands = [
        insert(),
        EditorCommand::UpdateNode {
            node_id: NodeId::new("n11"),
            x: Some(43),
            y: Some(27),
            width: None,
            height: None,
            name: Some("Changed".into()),
            fill_hex: None,
            page_id: None,
        },
        EditorCommand::DeleteNode {
            node_id: NodeId::new("n11"),
            page_id: None,
        },
    ];
    for command in commands {
        let before = state.doc.clone();
        let pushes = state.history_push_count;
        assert!(state.apply_undoable(command));
        let after = state.doc.clone();
        assert_ne!(after, before);
        assert_eq!(state.history_push_count, pushes + 1);
        assert!(state.undo());
        assert_eq!(state.doc, before);
        assert!(state.redo());
        assert_eq!(state.doc, after);
    }
}

#[test]
fn existing_history_and_nested_maintenance_coalesce_once() {
    let mut state = sample();
    let before = state.doc.clone();
    assert!(state.apply_undoable_then(nudge(1, 0), |state| {
        assert!(state.apply_undoable(nudge(0, 2)));
    }));
    assert_eq!(state.history.past.len(), 1);
    assert_eq!(state.history_push_count, 1);
    assert!(state.undo());
    assert_eq!(state.doc, before);
}

#[test]
fn atomic_batch_inside_a_command_boundary_is_one_exact_step() {
    let mut state = sample();
    let before = state.doc.clone();
    assert!(state.apply_undoable(EditorCommand::Batch {
        commands: vec![nudge(1, 2), insert()],
    }));
    assert_eq!(state.history.past.len(), 1);
    assert_eq!(state.history_push_count, 1);
    assert!(!state.undo_capture_suspended);
    assert!(state.undo());
    assert_eq!(state.doc, before);
    let future = state.history.future.clone();
    assert!(!state.apply_undoable(EditorCommand::Batch {
        commands: vec![
            nudge(1, 0),
            EditorCommand::DeleteNode {
                node_id: NodeId::new("missing"),
                page_id: None,
            }
        ],
    }));
    assert_eq!(state.doc, before);
    assert!(state.history.past.is_empty());
    assert_eq!(state.history.future, future);
    assert!(!state.undo_capture_suspended);
}

#[test]
fn view_and_clipboard_calls_preserve_redo_and_document_revision() {
    let mut state = sample();
    assert!(state.apply_undoable(nudge(1, 0)));
    assert!(state.undo());
    let before = state.doc.clone();
    let future = state.history.future.clone();
    let revision = state.document_revision();
    assert!(state.apply_undoable(EditorCommand::SetViewport {
        pan_x: Some(40),
        pan_y: Some(80),
        zoom_percent: Some(125),
    }));
    assert!(state.apply_undoable(EditorCommand::CopySelected));
    assert_eq!(state.doc, before);
    assert_eq!(state.document_revision(), revision);
    assert!(state.history.past.is_empty());
    assert_eq!(state.history.future, future);
}

#[test]
fn panicking_group_preserves_one_recovery_step_and_restores_scope() {
    let mut state = sample();
    let before = state.doc.clone();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.with_history_group(|state| {
            assert!(state.apply(nudge(1, 0)));
            assert!(state.apply(nudge(0, 2)));
            panic!("maintenance probe");
        });
    }));
    assert!(panic.is_err());
    assert!(!state.undo_capture_suspended);
    assert_eq!(state.history.past.len(), 1);
    assert!(state.undo());
    assert_eq!(state.doc, before);
}

#[test]
fn allocator_failure_does_not_erase_redo_or_add_history() {
    let mut state = sample();
    assert!(state.apply_undoable(nudge(1, 0)));
    assert!(state.undo());
    let before = state.doc.clone();
    let future = state.history.future.clone();
    let mut allocator = DocumentIdAllocator::sequential(u64::MAX);
    assert_eq!(
        state.apply_undoable_with_allocator(insert(), &mut allocator),
        Err(IdAllocError::CounterExhausted)
    );
    assert_eq!(state.doc, before);
    assert!(state.history.past.is_empty());
    assert_eq!(state.history.future, future);
    assert!(!state.undo_capture_suspended);
}
