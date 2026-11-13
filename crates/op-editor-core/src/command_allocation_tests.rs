use super::*;
use crate::{
    test_support::{frame, rect, state_with},
    IdAllocator, NodeId, PenNodeExt,
};

fn insert() -> EditorCommand {
    EditorCommand::InsertNode {
        kind: "rect".into(),
        name: "Added".into(),
        x: 0,
        y: 0,
        width: 4,
        height: 4,
        fill_hex: None,
        target_parent: NodeId::NONE,
        page_id: None,
    }
}

#[test]
fn existing_node_edits_and_deletes_work_when_new_numeric_ids_are_exhausted() {
    let max = NodeId::new(format!("n{}", u64::MAX));
    let mut state = state_with(vec![rect(max.as_str(), "Existing", 0.0, 0.0, 8.0, 8.0)]);
    for command in [
        EditorCommand::UpdateNode {
            node_id: max.clone(),
            x: Some(12),
            y: Some(20),
            width: Some(10),
            height: Some(11),
            name: None,
            fill_hex: None,
            page_id: None,
        },
        EditorCommand::SetNodeName {
            node_id: max.clone(),
            name: "Renamed".into(),
        },
        EditorCommand::SetNodeFillHex {
            node_id: max.clone(),
            hex: "#123456".into(),
        },
        EditorCommand::SetNodeRotation {
            node_id: max.clone(),
            degrees: 30.0,
        },
        EditorCommand::AddNodeEffect {
            node_id: max.clone(),
            kind: "shadow".into(),
        },
    ] {
        assert!(state.apply(command));
    }
    let node = &state.active_children()[0];
    assert_eq!(node.base().name.as_deref(), Some("Renamed"));
    assert_eq!((node.base().x, node.base().y), (Some(12.0), Some(20.0)));
    assert_eq!(node.base().rotation, Some(30.0));
    assert!(!state.apply(insert()));
    assert!(state.apply(EditorCommand::DeleteNode {
        node_id: max,
        page_id: None
    }));
    assert!(state.active_children().is_empty());
    assert!(state.apply(insert()));
    assert_eq!(state.active_children()[0].id_str(), "n1");
}

#[test]
fn batches_without_allocations_allow_updates_at_counter_exhaustion() {
    let max = NodeId::new(format!("n{}", u64::MAX));
    let mut state = state_with(vec![rect(max.as_str(), "Old", 0.0, 0.0, 8.0, 8.0)]);
    assert!(state.apply(EditorCommand::Batch {
        commands: vec![
            EditorCommand::SetNodeName {
                node_id: max.clone(),
                name: "New".into()
            },
            EditorCommand::SetNodeRotation {
                node_id: max,
                degrees: 45.0,
            },
        ]
    }));
    assert_eq!(
        state.active_children()[0].base().name.as_deref(),
        Some("New")
    );
    assert_eq!(state.active_children()[0].base().rotation, Some(45.0));
}

#[test]
fn mixed_batches_keep_the_pre_command_seed_and_complete_collision_domain() {
    let mut state = state_with(vec![rect("n100", "Old", 0.0, 0.0, 8.0, 8.0)]);
    assert!(state.apply(EditorCommand::Batch {
        commands: vec![
            EditorCommand::DeleteNode {
                node_id: NodeId::new("n100"),
                page_id: None
            },
            insert(),
        ]
    }));
    assert_eq!(state.active_children()[0].id_str(), "n101");

    let mut state = state_with(vec![]);
    assert!(state.apply(EditorCommand::Batch {
        commands: vec![
            EditorCommand::InsertAuthoredSubtreePreservingRoots {
                nodes: vec![rect("n1", "Authored", 0.0, 0.0, 8.0, 8.0)],
                parent_id: NodeId::NONE,
                page_id: None,
            },
            insert(),
        ]
    }));
    let ids: Vec<_> = state
        .active_children()
        .iter()
        .map(PenNodeExt::id_str)
        .collect();
    assert_eq!(ids, ["n1", "n2"]);
}

#[test]
fn exhausted_allocating_batch_does_not_commit_an_earlier_edit() {
    let max = NodeId::new(format!("n{}", u64::MAX));
    let mut state = state_with(vec![rect(max.as_str(), "Old", 0.0, 0.0, 8.0, 8.0)]);
    let before = state.doc.clone();
    assert!(!state.apply(EditorCommand::Batch {
        commands: vec![
            EditorCommand::SetNodeName {
                node_id: max,
                name: "Must roll back".into()
            },
            insert(),
        ]
    }));
    assert_eq!(state.doc, before);
}

#[test]
fn standalone_leaf_ids_stay_above_all_pages_and_nested_nodes() {
    let mut state = state_with(vec![rect("n7", "Legacy root", 0.0, 0.0, 8.0, 8.0)]);
    state.doc.pages = Some(vec![jian_ops_schema::page::PenPage {
        id: "n50".into(),
        name: "Page".into(),
        background_color: None,
        state: None,
        lifecycle: None,
        children: vec![frame(
            "n20",
            "Container",
            0.0,
            0.0,
            20.0,
            20.0,
            vec![rect("n00080", "Nested", 0.0, 0.0, 8.0, 8.0)],
        )],
    }]);
    assert!(state.apply(insert()));
    assert_eq!(state.active_children().last().unwrap().id_str(), "n81");
    let item = crate::command::BatchInsertItem {
        kind: "rect".into(),
        name: "Batch".into(),
        x: 0,
        y: 0,
        width: 4,
        height: 4,
        fill_hex: None,
        fill: None,
    };
    assert!(state.apply(EditorCommand::BatchInsert {
        items: vec![item.clone(), item],
        page_id: None
    }));
    let ids: Vec<_> = state
        .active_children()
        .iter()
        .map(PenNodeExt::id_str)
        .collect();
    assert_eq!(ids, ["n20", "n81", "n82", "n83"]);
}

#[test]
fn external_allocators_still_receive_every_existing_page_and_node_id() {
    struct External;
    impl IdAllocator for External {
        fn allocate(
            &mut self,
            taken: &mut std::collections::HashSet<NodeId>,
        ) -> Result<NodeId, crate::IdAllocError> {
            for id in ["n1", "n2", "n5", "n9"] {
                assert!(taken.contains(&NodeId::new(id)), "missing {id}");
            }
            let id = NodeId::new("external_fresh");
            assert!(taken.insert(id.clone()));
            Ok(id)
        }
    }
    let mut state = state_with(vec![rect("n1", "Root", 0.0, 0.0, 8.0, 8.0)]);
    state.doc.pages = Some(vec![jian_ops_schema::page::PenPage {
        id: "n9".into(),
        name: "Page".into(),
        background_color: None,
        state: None,
        lifecycle: None,
        children: vec![frame(
            "n2",
            "Container",
            0.0,
            0.0,
            20.0,
            20.0,
            vec![rect("n5", "Nested", 0.0, 0.0, 8.0, 8.0)],
        )],
    }]);
    assert_eq!(
        state.apply_with_allocator(insert(), &mut External),
        Ok(true)
    );
    assert_eq!(
        state.active_children().last().unwrap().id_str(),
        "external_fresh"
    );
}
