//! `EditorCommand::InsertAuthoredSubtree` tests.

#![cfg(test)]

use crate::command::EditorCommand;
use crate::node_id::NodeId;
use crate::pen_node_ext::PenNodeExt;
use crate::test_support::{frame, rect, state_with};

#[test]
fn insert_authored_subtree_rejects_empty_authored_id() {
    // Authored subtrees require a valid (non-empty) id on every node — the
    // ONLY accept/reject difference vs `cmd_insert_subtree` (which remaps and
    // so tolerates an empty id) besides id collisions. Callers that mint ids
    // (batch_design) never hit this; a malformed empty id is rejected.
    let mut s = state_with(vec![]);
    let mut node = rect("x", "X", 0.0, 0.0, 10.0, 10.0);
    node.base_mut().id = String::new();

    assert!(!s.apply(EditorCommand::InsertAuthoredSubtree {
        nodes: vec![node],
        parent_id: NodeId::NONE,
        page_id: None,
    }));
    assert!(s.active_children().is_empty());
}

#[test]
fn insert_authored_subtree_preserves_ids_for_layered_workflow() {
    let mut s = state_with(vec![]);

    assert!(s.apply(EditorCommand::InsertAuthoredSubtree {
        nodes: vec![frame(
            "root",
            "Root",
            0.0,
            0.0,
            375.0,
            812.0,
            vec![rect("hero", "Hero", 0.0, 0.0, 375.0, 240.0)],
        )],
        parent_id: NodeId::NONE,
        page_id: None,
    }));

    assert_eq!(s.active_children()[0].id_str(), "root");
    let section = &s.active_children()[0].children().expect("children")[0];
    assert_eq!(section.id_str(), "hero");
}

#[test]
fn insert_authored_root_frame_replaces_empty_root_frame() {
    let mut s = state_with(vec![frame(
        "default",
        "Frame",
        30.0,
        40.0,
        100.0,
        100.0,
        vec![],
    )]);

    assert!(s.apply(EditorCommand::InsertAuthoredSubtree {
        nodes: vec![frame(
            "food-home",
            "Food App Home",
            0.0,
            0.0,
            402.0,
            874.0,
            vec![rect("hero", "Hero", 0.0, 0.0, 402.0, 120.0)],
        )],
        parent_id: NodeId::NONE,
        page_id: None,
    }));

    let children = s.active_children();
    assert_eq!(children.len(), 1, "empty default frame should be replaced");
    assert_eq!(children[0].id_str(), "food-home");
    assert_eq!(children[0].base().x, Some(30.0));
    assert_eq!(children[0].base().y, Some(40.0));
}

#[test]
fn insert_authored_subtree_accepts_empty_container_parent() {
    // Regression: a container whose `children` is still `None` (here a `rect`,
    // which is a container) must accept an insert — `cmd_insert_subtree` does,
    // and the old `children().is_some()` check wrongly rejected it.
    let mut s = state_with(vec![rect("box", "Box", 0.0, 0.0, 100.0, 100.0)]);

    assert!(s.apply(EditorCommand::InsertAuthoredSubtree {
        nodes: vec![rect("child", "Child", 0.0, 0.0, 10.0, 10.0)],
        parent_id: NodeId::new("box"),
        page_id: None,
    }));

    let parent = &s.active_children()[0];
    assert_eq!(parent.id_str(), "box");
    let inserted = &parent.children().expect("children initialized")[0];
    assert_eq!(inserted.id_str(), "child");
}

#[test]
fn insert_authored_subtree_rejects_live_id_collision() {
    let mut s = state_with(vec![rect("root", "Existing", 0.0, 0.0, 10.0, 10.0)]);

    assert!(!s.apply(EditorCommand::InsertAuthoredSubtree {
        nodes: vec![rect("root", "Duplicate", 0.0, 0.0, 10.0, 10.0)],
        parent_id: NodeId::NONE,
        page_id: None,
    }));

    assert_eq!(s.active_children().len(), 1);
    assert_eq!(
        s.active_children()[0].base().name.as_deref(),
        Some("Existing")
    );
}

#[test]
fn authored_fresh_ids_check_every_page_root_and_incoming_descendant() {
    let mut document = state_with(vec![rect("n50", "Root", 0.0, 0.0, 4.0, 4.0)]).doc;
    document.pages = Some(vec![
        jian_ops_schema::page::PenPage {
            id: "n900".into(),
            name: "One".into(),
            children: vec![rect("n700", "Page child", 0.0, 0.0, 4.0, 4.0)],
            background_color: None,
            state: None,
            lifecycle: None,
        },
        jian_ops_schema::page::PenPage {
            id: "other".into(),
            name: "Two".into(),
            children: vec![rect("n800", "Other page", 0.0, 0.0, 4.0, 4.0)],
            background_color: None,
            state: None,
            lifecycle: None,
        },
    ]);
    let original = crate::EditorState::from_document(document);
    for collision in ["n50", "n700", "n800", "n900"] {
        let mut state = original.clone();
        let before = serde_json::to_vec(&state.doc).unwrap();
        assert!(
            !state.apply(EditorCommand::InsertAuthoredSubtreePreservingRoots {
                nodes: vec![frame(
                    "n901",
                    "Incoming",
                    0.0,
                    0.0,
                    20.0,
                    20.0,
                    vec![rect(collision, "Collision", 0.0, 0.0, 4.0, 4.0)]
                )],
                parent_id: NodeId::NONE,
                page_id: None,
            }),
            "{collision}"
        );
        assert_eq!(serde_json::to_vec(&state.doc).unwrap(), before);
        assert!(state.history.past.is_empty());
    }
    let mut state = original;
    assert!(
        state.apply(EditorCommand::InsertAuthoredSubtreePreservingRoots {
            nodes: vec![frame(
                "n901",
                "Incoming",
                0.0,
                0.0,
                20.0,
                20.0,
                vec![rect("n902", "Fresh", 0.0, 0.0, 4.0, 4.0)]
            )],
            parent_id: NodeId::NONE,
            page_id: None,
        })
    );
    assert_eq!(state.active_children().last().unwrap().id_str(), "n901");
}

#[test]
fn authored_numeric_proof_keeps_exact_string_identity_and_duplicate_validation() {
    let mut state = state_with(vec![rect("n0010", "Original", 0.0, 0.0, 4.0, 4.0)]);
    for id in ["n10", "custom-Ω", "n11"] {
        assert!(
            state.apply(EditorCommand::InsertAuthoredSubtreePreservingRoots {
                nodes: vec![rect(id, "Added", 0.0, 0.0, 4.0, 4.0)],
                parent_id: NodeId::NONE,
                page_id: None,
            })
        );
    }
    let before = serde_json::to_vec(&state.doc).unwrap();
    for child in ["n20", ""] {
        let mut descendant = rect("valid", "Invalid descendant", 0.0, 0.0, 4.0, 4.0);
        descendant.base_mut().id = child.into();
        assert!(
            !state.apply(EditorCommand::InsertAuthoredSubtreePreservingRoots {
                nodes: vec![frame(
                    "n20",
                    "Duplicate or empty",
                    0.0,
                    0.0,
                    20.0,
                    20.0,
                    vec![descendant]
                )],
                parent_id: NodeId::NONE,
                page_id: None,
            })
        );
        assert_eq!(serde_json::to_vec(&state.doc).unwrap(), before);
    }
}

#[test]
fn authored_inserts_do_not_need_an_available_allocation_counter() {
    let maximum = format!("n{}", u64::MAX);
    let mut state = state_with(vec![rect(&maximum, "At max", 0.0, 0.0, 4.0, 4.0)]);
    let before = serde_json::to_vec(&state.doc).unwrap();
    assert!(state.apply_undoable(EditorCommand::Batch {
        commands: vec![
            EditorCommand::InsertAuthoredSubtreePreservingRoots {
                nodes: vec![rect("external-id", "Authored", 0.0, 0.0, 4.0, 4.0)],
                parent_id: NodeId::NONE,
                page_id: None,
            },
            EditorCommand::UpdateNode {
                node_id: NodeId::new(&maximum),
                x: Some(12),
                y: None,
                width: None,
                height: None,
                name: None,
                fill_hex: None,
                page_id: None
            },
        ]
    }));
    let after = serde_json::to_vec(&state.doc).unwrap();
    assert!(state.apply(EditorCommand::Undo));
    assert_eq!(serde_json::to_vec(&state.doc).unwrap(), before);
    assert!(state.apply(EditorCommand::Redo));
    assert_eq!(serde_json::to_vec(&state.doc).unwrap(), after);
    assert!(!state.apply(EditorCommand::InsertNode {
        kind: "rect".into(),
        name: "Must allocate".into(),
        x: 0,
        y: 0,
        width: 4,
        height: 4,
        fill_hex: None,
        target_parent: NodeId::NONE,
        page_id: None
    }));
}
