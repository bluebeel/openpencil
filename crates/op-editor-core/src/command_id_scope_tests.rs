use super::*;
use crate::{
    test_support::{frame, rect, state_with},
    PenNodeExt,
};

fn authored(id: &str) -> EditorCommand {
    EditorCommand::InsertAuthoredSubtreePreservingRoots {
        nodes: vec![rect(id, "authored", 0.0, 0.0, 8.0, 8.0)],
        parent_id: NodeId::NONE,
        page_id: None,
    }
}
fn delete(id: &str) -> EditorCommand {
    EditorCommand::DeleteNode {
        node_id: NodeId::new(id),
        page_id: None,
    }
}
fn patch(id: &str, value: serde_json::Value) -> EditorCommand {
    EditorCommand::PatchNodeData {
        node_id: NodeId::new(id),
        patch_json: value.to_string(),
        page_id: None,
    }
}
fn check_sequence(commands: Vec<EditorCommand>) {
    let mut reference = state_with(vec![frame(
        "holder",
        "Holder",
        0.0,
        0.0,
        8.0,
        8.0,
        vec![rect("n8", "Existing", 0.0, 0.0, 8.0, 8.0)],
    )]);
    let mut candidate = CommandSimulation::new(reference.clone());
    let mut minted = 0;
    for command in commands {
        let expected = reference.apply(command.clone());
        assert_eq!(candidate.apply(command), expected);
        assert_eq!(
            serde_json::to_vec(&candidate.doc).unwrap(),
            serde_json::to_vec(&reference.doc).unwrap()
        );
        let seed = reference.next_node_id_seed().map(|n| n.max(minted));
        assert_eq!(candidate.next_program_id_seed(minted), seed);
        if let Some(seed) = seed {
            minted = seed;
        }
    }
}
#[test]
fn owned_sequence_matches_fresh_validation_after_id_mutations_and_failures() {
    check_sequence(vec![
        authored("n9"),
        delete("n9"),
        authored("n9"),
        authored("n9"),
        authored("custom"),
        delete("custom"),
        authored("custom"),
        patch("n9", serde_json::json!({"id":"n8000"})),
        authored("n8000"),
        authored("n8001"),
        patch(
            "holder",
            serde_json::json!({"children":[{"type":"rectangle","id":"n9000"}]}),
        ),
        authored("n9000"),
        authored("n9001"),
        patch(
            "holder",
            serde_json::json!({"children":[{"type":"rectangle","id":"n18446744073709551615"}]}),
        ),
        authored("low-custom"),
        authored("n9002"),
        delete("holder"),
        authored("n9003"),
    ]);
}
#[test]
fn conservative_floor_allows_reinsert_and_atomic_collision_rollback() {
    let commands = vec![
        authored("n40"),
        delete("n40"),
        authored("n2"),
        authored("custom"),
        delete("custom"),
        authored("custom"),
    ];
    let mut reference = state_with(vec![]);
    let mut allocator = DocumentIdAllocator::sequential(1);
    for command in &commands {
        assert!(reference
            .apply_with_allocator(command.clone(), &mut allocator)
            .unwrap());
    }
    let mut candidate = state_with(vec![]);
    assert!(candidate.apply(EditorCommand::Batch { commands }));
    assert_eq!(
        serde_json::to_vec(&candidate.doc).unwrap(),
        serde_json::to_vec(&reference.doc).unwrap()
    );
    let before = serde_json::to_vec(&candidate.doc).unwrap();
    let history = candidate.history.past.len();
    assert!(!candidate.apply(EditorCommand::Batch {
        commands: vec![
            authored("n100"),
            patch("n100", serde_json::json!({"id":"n9000"})),
            authored("n9000")
        ]
    }));
    assert_eq!(serde_json::to_vec(&candidate.doc).unwrap(), before);
    assert_eq!(candidate.history.past.len(), history);
    assert!(candidate.undo());
    assert!(candidate.active_children().is_empty());
}
#[test]
fn public_mutation_between_owned_scopes_never_reuses_an_old_proof() {
    let mut first = CommandSimulation::new(state_with(vec![]));
    assert_eq!(first.next_program_id_seed(0), Some(1));
    assert!(first.apply(authored("n1")));
    let mut state = first.into_inner();
    state
        .doc
        .children
        .push(rect("n700", "Public write", 0.0, 0.0, 8.0, 8.0));
    let mut next = CommandSimulation::new(state);
    assert_eq!(next.next_program_id_seed(0), Some(701));
    assert!(!next.apply(authored("n700")));
    assert!(next.apply(authored("n701")));
}
#[test]
fn numeric_page_ids_and_namespace_allocator_keep_complete_collision_domain() {
    let mut doc: jian_ops_schema::PenDocument = serde_json::from_value(serde_json::json!({
        "version":"1.0.0","children":[],"pages":[{"id":"n900","name":"Page","children":[]}]
    }))
    .unwrap();
    let mut state = EditorState::from_document(doc.clone());
    let namespace = crate::PeerNamespace::parse("peer").unwrap();
    let mut allocator = DocumentIdAllocator::namespaced(namespace, 1);
    assert!(!state
        .apply_with_allocator(
            EditorCommand::Batch {
                commands: vec![authored("n900")]
            },
            &mut allocator
        )
        .unwrap());
    assert!(state
        .apply_with_allocator(
            EditorCommand::Batch {
                commands: vec![authored("n901")]
            },
            &mut allocator
        )
        .unwrap());
    assert_eq!(state.active_children()[0].id_str(), "n901");
    doc.pages.as_mut().unwrap()[0].children.clear();
}
