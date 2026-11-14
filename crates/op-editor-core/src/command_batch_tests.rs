//! `EditorCommand::Batch` atomic-apply tests.

#![cfg(test)]

use crate::command::EditorCommand;
use crate::node_id::NodeId;
use crate::pen_node_ext::PenNodeExt;
use crate::test_support::sample;

fn insert(name: &str) -> EditorCommand {
    EditorCommand::InsertNode {
        kind: "rect".into(),
        name: name.into(),
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
fn batch_applies_every_sub_command_in_order() {
    let mut s = sample();
    let before = s.active_children().len();
    assert!(s.apply(EditorCommand::Batch {
        commands: vec![
            insert("A"),
            EditorCommand::SetNodeName {
                node_id: NodeId::new("n11"),
                name: "Renamed".into(),
            },
        ],
    }));
    assert_eq!(s.active_children().len(), before + 1);
    let title = crate::walkers::find_node(s.active_children(), &NodeId::new("n11")).unwrap();
    assert_eq!(title.base().name.as_deref(), Some("Renamed"));
}

#[test]
fn batch_rolls_back_on_mid_batch_failure() {
    let mut s = sample();
    // Seed one undo entry AND one redo entry so the depths are
    // observable across the failed batch. Plain `InsertNode` pushes
    // no history at apply time (the Batch wrapper / host owns that),
    // so seed with selection-scoped edits, which do.
    assert!(s.apply(EditorCommand::NudgeSelected { dx: 1, dy: 0 }));
    assert!(s.apply(EditorCommand::NudgeSelected { dx: 0, dy: 1 }));
    assert!(s.undo());
    let before_doc = serde_json::to_string(&s.doc).unwrap();
    let before_selection = s.selection.clone();
    assert_eq!(s.history.past.len(), 1);
    assert_eq!(s.history.future.len(), 1);
    assert!(!s.apply(EditorCommand::Batch {
        commands: vec![
            insert("A"),
            // Unknown node id — rejects at apply time.
            EditorCommand::DeleteNode {
                node_id: NodeId::new("nope"),
                page_id: None,
            },
        ],
    }));
    assert_eq!(
        serde_json::to_string(&s.doc).unwrap(),
        before_doc,
        "a failed batch must leave the document untouched"
    );
    assert_eq!(
        s.selection, before_selection,
        "a failed batch must leave the selection untouched"
    );
    assert_eq!(
        s.history.past.len(),
        1,
        "a failed batch must leave the undo depth untouched"
    );
    assert_eq!(
        s.history.future.len(),
        1,
        "a failed batch must leave the redo stack untouched"
    );
}

#[test]
fn batch_rejects_empty_and_nested_batches() {
    let mut s = sample();
    assert!(!s.apply(EditorCommand::Batch { commands: vec![] }));
    let before_doc = serde_json::to_string(&s.doc).unwrap();
    assert!(!s.apply(EditorCommand::Batch {
        commands: vec![
            insert("A"),
            EditorCommand::Batch {
                commands: vec![insert("B")],
            },
        ],
    }));
    assert_eq!(
        serde_json::to_string(&s.doc).unwrap(),
        before_doc,
        "a nested batch must reject the outer batch up front"
    );
}

fn nudge() -> EditorCommand {
    EditorCommand::NudgeSelected { dx: 1, dy: 0 }
}

#[test]
fn large_batch_preserves_older_history_at_capacity_and_has_one_undo_step() {
    let mut s = sample();
    for _ in 0..crate::HISTORY_CAP {
        assert!(s.apply(nudge()));
    }
    let before_doc = s.doc.clone();
    let mut expected_history = s.history.past.clone();
    expected_history.pop_front();
    let pushed = s.history_push_count;
    assert!(s.apply(EditorCommand::Batch {
        commands: (0..crate::HISTORY_CAP + 20).map(|_| nudge()).collect(),
    }));
    assert_eq!(s.history_push_count, pushed + 1);
    assert_eq!(s.history.past.len(), crate::HISTORY_CAP);
    assert!(s.history.future.is_empty());
    let after_doc = s.doc.clone();
    assert!(s.undo());
    assert_eq!(s.doc, before_doc);
    assert_eq!(s.history.past, expected_history);
    assert!(s.redo());
    assert_eq!(s.doc, after_doc);
}

#[test]
fn failed_large_batch_restores_every_old_history_entry_and_redo() {
    let mut s = sample();
    for _ in 0..crate::HISTORY_CAP + 1 {
        assert!(s.apply(nudge()));
    }
    assert!(s.undo());
    let before = s.snapshot_for_history();
    let history = s.history.clone();
    let mut commands: Vec<_> = (0..crate::HISTORY_CAP + 20).map(|_| nudge()).collect();
    commands.push(EditorCommand::DeleteNode {
        node_id: NodeId::new("missing"),
        page_id: None,
    });
    assert!(!s.apply(EditorCommand::Batch { commands }));
    assert_eq!(s.snapshot_for_history(), before);
    assert_eq!(s.history.past, history.past);
    assert_eq!(s.history.future, history.future);
    assert!(!s.undo_capture_suspended);
}

#[test]
fn selection_only_batch_keeps_redo_and_dirty_revision() {
    let mut s = sample();
    assert!(s.apply(nudge()));
    assert!(s.undo());
    let revision = s.revision;
    let future = s.history.future.clone();
    assert!(s.apply(EditorCommand::Batch {
        commands: vec![EditorCommand::ClearSelection],
    }));
    assert_eq!(s.revision, revision);
    assert_eq!(s.history.future, future);
    assert!(s.history.past.is_empty());
    assert!(s.selection.set.is_empty());
}

#[test]
fn panicking_batch_restores_document_and_history_then_resumes_unwind() {
    struct PanicAllocator;
    impl crate::IdAllocator for PanicAllocator {
        fn allocate(
            &mut self,
            _: &mut std::collections::HashSet<NodeId>,
        ) -> Result<NodeId, crate::IdAllocError> {
            panic!("allocator probe");
        }
    }
    let mut s = sample();
    assert!(s.apply(nudge()));
    let before = s.snapshot_for_history();
    let history = s.history.clone();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        s.apply_with_allocator(
            EditorCommand::Batch {
                commands: vec![nudge(), insert("unreachable")],
            },
            &mut PanicAllocator,
        )
    }));
    assert!(panicked.is_err());
    assert_eq!(s.snapshot_for_history(), before);
    assert_eq!(s.history.past, history.past);
    assert_eq!(s.history.future, history.future);
    assert!(!s.undo_capture_suspended);
    assert!(s.apply(nudge()));
    assert_eq!(s.history.past.len(), history.past.len() + 1);
}

#[test]
fn batch_rejects_history_sub_commands_up_front_with_no_state_change() {
    // Undo/redo exchange the surrounding stacks rather than one operation's
    // actor/document state, so they remain unsupported inside a Batch.
    let offenders = vec![EditorCommand::Undo, EditorCommand::Redo];
    for offender in offenders {
        let mut s = sample();
        let before_doc = serde_json::to_string(&s.doc).unwrap();
        let before_selection = s.selection.clone();
        let before_tool = s.tool;
        let before_viewport = s.viewport;
        assert!(
            !s.apply(EditorCommand::Batch {
                commands: vec![insert("A"), offender.clone(), insert("B")],
            }),
            "batch containing {offender:?} must be rejected"
        );
        assert_eq!(
            serde_json::to_string(&s.doc).unwrap(),
            before_doc,
            "rejecting {offender:?} must not run the preceding insert"
        );
        assert_eq!(s.selection, before_selection);
        assert_eq!(s.tool, before_tool);
        assert_eq!(s.viewport, before_viewport);
        assert!(s.clipboard.is_empty(), "clipboard must stay untouched");
        assert!(s.history.past.is_empty(), "no history entry may land");
        assert!(s.history.future.is_empty());
    }
}

#[test]
fn batch_rejection_runs_nothing_even_when_the_offender_leads() {
    let mut s = sample();
    let before = s.active_children().len();
    // History exchange rejects before the following insert can run.
    assert!(!s.apply(EditorCommand::Batch {
        commands: vec![EditorCommand::Undo, insert("A"),],
    }));
    assert_eq!(s.tool, crate::tool::Tool::Select);
    assert_eq!(s.active_children().len(), before);
    assert!(s.history.past.is_empty());
}

#[test]
fn batch_lands_as_a_single_undo_step_and_failure_preserves_redo() {
    let mut s = sample();
    let before_doc = serde_json::to_string(&s.doc).unwrap();
    assert!(s.apply(EditorCommand::Batch {
        commands: vec![insert("A"), insert("B")],
    }));
    let after_doc = serde_json::to_string(&s.doc).unwrap();
    assert_ne!(before_doc, after_doc);
    // One undo reverts the WHOLE batch.
    assert!(s.undo());
    assert_eq!(serde_json::to_string(&s.doc).unwrap(), before_doc);
    // Redo stack now holds the batch; a FAILED batch must not eat it.
    assert!(!s.apply(EditorCommand::Batch {
        commands: vec![EditorCommand::DeleteNode {
            node_id: NodeId::new("nope"),
            page_id: None,
        }],
    }));
    assert!(s.redo(), "redo must survive a rolled-back batch");
    assert_eq!(serde_json::to_string(&s.doc).unwrap(), after_doc);
}
