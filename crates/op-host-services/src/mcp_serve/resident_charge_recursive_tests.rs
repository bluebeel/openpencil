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
    assert!(
        retained < initial * 3,
        "shared history must be charged once per recursive allocation: {retained}/{initial}"
    );
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
