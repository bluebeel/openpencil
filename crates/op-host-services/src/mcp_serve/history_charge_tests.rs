use super::*;
use op_editor_core::{EditorCommand, NodeId, SharedDoc};
use serde_json::json;
fn fixture() -> EditorState {
    let doc = serde_json::from_value(json!({"version":"1.0.0","children":[{"type":"frame","id":"root","children":[{"type":"text","id":"a","content":"complete historical metadata","fontFeatures":{"salt":1},"textDecorationSkipInk":false},{"type":"rectangle","id":"b","width":8,"height":8}]}],"pages":[{"id":"p","name":"page","children":[{"type":"rectangle","id":"page-leaf","width":9,"height":8}]}]})).unwrap();
    EditorState::from_document(doc)
}
fn update(id: &str, x: i32) -> EditorCommand {
    EditorCommand::UpdateNode {
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
fn legacy_bytes(cache: &ChargeCache) -> usize {
    cache.nodes.retained_bytes()
        + cache.documents.retained_bytes()
        + cache.components.retained_bytes()
}
fn equivalent(state: &EditorState, cache: &mut ChargeCache) -> usize {
    let mut reference = ChargeCache::default();
    let old = resident_charge_mode(state, 1_000_000_000, &mut reference, false).unwrap();
    let current = resident_charge_cached(state, 1_000_000_000, cache).unwrap();
    assert_eq!(
        current - cache.history.retained_bytes() - legacy_bytes(cache),
        old - legacy_bytes(&reference),
        "retained allocations exact, cache storage charged separately"
    );
    current
}
#[test]
fn history_union_tracks_edits_undo_redo_replacement_and_drops_background_weak_entries() {
    let mut state = fixture();
    let mut cache = ChargeCache::default();
    state.history_push_past(state.snapshot_for_history());
    equivalent(&state, &mut cache);
    for i in 0..8 {
        assert!(state.apply_undoable(update("page-leaf", i)));
        equivalent(&state, &mut cache);
    }
    let charge = equivalent(&state, &mut cache);
    assert_eq!(cache.history.additions, 0);
    assert_eq!(charge, equivalent(&state, &mut cache));
    assert!(state.undo());
    equivalent(&state, &mut cache);
    equivalent(&state, &mut cache);
    assert_eq!(
        cache.history.additions, 0,
        "unchanged post-undo history reuses its union"
    );
    assert!(state.redo());
    equivalent(&state, &mut cache);
    equivalent(&state, &mut cache);
    assert_eq!(cache.history.additions, 0);
    let background = state.history.past.front().unwrap().doc.root_children()[0].clone();
    let mut replacement = state.doc.clone();
    replacement.name = Some("replacement metadata with larger storage".repeat(50));
    state.history.past.front_mut().unwrap().doc = SharedDoc::capture(&replacement, None);
    equivalent(&state, &mut cache);
    assert!(cache.history.additions > 0);
    state.history.past.clear();
    state.history.future.clear();
    equivalent(&state, &mut cache);
    assert_eq!(cache.history.retained_bytes(), 0);
    assert_eq!(
        Arc::weak_count(&background),
        0,
        "background saves must not keep obsolete cache slots"
    );
}
#[test]
fn history_cache_capacity_and_tight_authority_limits_fall_back_without_undercharging() {
    let mut state = fixture();
    for _ in 0..4 {
        state.history_push_past(state.snapshot_for_history());
    }
    let mut reference = ChargeCache::default();
    let minimum = resident_charge_mode(&state, 1_000_000_000, &mut reference, false).unwrap();
    let mut limited = ChargeCache::default();
    limited.history.set_test_budget(512, 2);
    assert_eq!(
        resident_charge_cached(&state, 1_000_000_000, &mut limited).unwrap(),
        minimum
    );
    assert_eq!(limited.history.retained_bytes(), 0);
    let mut cache = ChargeCache::default();
    let full = equivalent(&state, &mut cache);
    assert!(full > minimum);
    assert_eq!(
        resident_charge_cached(&state, minimum, &mut cache).unwrap(),
        minimum
    );
    assert_eq!(
        cache.history.retained_bytes(),
        0,
        "drop optional cache before rejecting a fitting authority"
    );
    assert!(matches!(
        resident_charge_cached(&state, minimum - 1, &mut cache),
        Err(ChargeError::OverLimit)
    ));
}
#[test]
fn historical_payload_limit_failure_clears_partial_cache_and_recovers() {
    let mut state = EditorState::new();
    let mut old = state.doc.clone();
    old.name = Some("retained".repeat(200_000));
    let mut snapshot = state.snapshot_for_history();
    snapshot.doc = SharedDoc::capture(&old, None);
    state.history_push_past(snapshot);
    let mut cache = ChargeCache::default();
    assert!(matches!(
        resident_charge_cached(&state, 128 * 1024, &mut cache),
        Err(ChargeError::OverLimit)
    ));
    assert_eq!(cache.history.retained_bytes(), 0);
    state.history.past.clear();
    assert!(resident_charge_cached(&state, 128 * 1024, &mut cache).is_ok());
}
