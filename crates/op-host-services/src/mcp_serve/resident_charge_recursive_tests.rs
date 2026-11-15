use super::*;
use op_editor_core::{EditorCommand, NodeId};
use serde_json::json;
#[test]
fn recursive_history_charge_deduplicates_metadata_nodes_and_releases_weak_cache() {
    let children: Vec<_> = (0..5000)
        .map(|i| json!({"type":"rectangle","id":format!("leaf{i}"),"x":0,"width":8,"height":8}))
        .collect();
    let doc: PenDocument = serde_json::from_value(
        json!({"version":"1.0.0","children":[{"type":"frame","id":"root","children":children}]}),
    )
    .unwrap();
    let mut state = EditorState::from_document(doc);
    let mut cache = ChargeCache::default();
    let initial = resident_charge_cached(&state, 1_000_000_000, &mut cache).unwrap();
    for step in 1..=100 {
        assert!(state.apply_undoable(EditorCommand::UpdateNode {
            node_id: NodeId::new("leaf4999"),
            x: Some(step),
            y: None,
            width: None,
            height: None,
            name: None,
            fill_hex: None,
            page_id: None
        }));
    }
    let retained = resident_charge_cached(&state, 1_000_000_000, &mut cache).unwrap();
    assert_eq!(
        retained,
        resident_charge_cached(&state, 1_000_000_000, &mut cache).unwrap()
    );
    // Inline nodes can shrink independently of the 100 immutable child-edge
    // arrays. Prove recursive sharing directly instead of comparing the ledger
    // against a size-dependent multiple of the current document.
    let snapshots: Vec<_> = state
        .history
        .past
        .iter()
        .map(|entry| entry.full_snapshot().expect("complete legacy snapshot"))
        .collect();
    assert_eq!(snapshots.len(), 100);
    let first = snapshots[0].doc.root_children()[0].children().unwrap();
    let mut metadata = HashSet::new();
    let mut child_edges = HashSet::new();
    for snapshot in &snapshots {
        let root = &snapshot.doc.root_children()[0];
        metadata.insert(Arc::as_ptr(root) as usize);
        child_edges.insert(root.child_edge_storage().unwrap().0);
        let children = root.children().unwrap();
        assert_eq!(children.len(), 5000);
        for (index, child) in children.iter().enumerate() {
            metadata.insert(Arc::as_ptr(child) as usize);
            if index < 4999 {
                assert!(Arc::ptr_eq(child, &first[index]));
            }
        }
    }
    assert_eq!(metadata.len(), 4999 + 2 * snapshots.len());
    assert_eq!(child_edges.len(), snapshots.len());
    assert!(retained > initial);
    assert!(retained < 200_000_000);
    state.history.past.clear();
    state.history.future.clear();
    let cleared = resident_charge_cached(&state, 1_000_000_000, &mut cache).unwrap();
    assert!(cleared < retained);
    assert_eq!(cache.nodes.retained_bytes(), 0);
    assert_eq!(cache.documents.retained_bytes(), 0);
    assert_eq!(cache.components.retained_bytes(), 0);
}

#[test]
fn checkpoint_edge_arrays_are_charged_once_across_flat_history_clones() {
    let children: Vec<_> = (0..5000)
        .map(|i| json!({"type":"rectangle","id":format!("leaf{i}"),"width":8,"height":8}))
        .collect();
    let doc: PenDocument =
        serde_json::from_value(json!({"version":"1.0.0","children":children})).unwrap();
    let mut state = EditorState::from_document(doc);
    let snapshot = state.snapshot_for_history();
    state.history_push_past(snapshot.clone());
    let mut cache = ChargeCache::default();
    let one = resident_charge_cached(&state, 1_000_000_000, &mut cache).unwrap();
    state.history_push_past(snapshot.clone());
    let two = resident_charge_cached(&state, 1_000_000_000, &mut cache).unwrap();
    assert!(two >= one);
    assert!(
        two - one < 4096,
        "another checkpoint shares every root-edge allocation: {one}/{two}"
    );
    for _ in 2..100 {
        state.history_push_past(snapshot.clone());
    }
    let hundred = resident_charge_cached(&state, 1_000_000_000, &mut cache).unwrap();
    assert!(hundred - one < 100 * 4096);
    assert_eq!(hundred, resident_charge(&state, 1_000_000_000).unwrap());
}
