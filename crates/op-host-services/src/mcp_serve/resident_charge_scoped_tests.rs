//! Scoped history is a real delta, never a partial document disguised as Full.

use super::*;
use op_editor_core::{EditorCommand, NodeId};
use serde_json::json;

fn fixture() -> EditorState {
    let document = serde_json::from_value(json!({"version":"1.0.0", "name":"complete metadata",
        "children":[{"type":"rectangle","id":"outside-page","width":1,"height":1}],
        "pages":[{"id":"p","name":"target page","children":[{"type":"frame","id":"root","children":[
            {"type":"rectangle","id":"target","width":8,"height":8,
                "events":{"onTap":[{"set":{"$state.v":1}}]},
                "semantics":{"label":"historical semantics"}},
            {"type":"text","id":"untouched","content":"retained complete text"}]}]},
        {"id":"unrelated","name":"unrelated page","children":[
            {"type":"frame","id":"absent-children"},
            {"type":"frame","id":"empty-children","children":[]}]}]}))
    .unwrap();
    EditorState::from_document(document)
}

fn apply_scoped(state: &mut EditorState, x: i32) {
    let command = EditorCommand::UpdateNode {
        node_id: NodeId::new("target"),
        x: Some(x),
        y: None,
        width: None,
        height: None,
        name: None,
        fill_hex: None,
        page_id: Some("p".into()),
    };
    let mut capture = state.begin_command_edit(&command);
    assert!(state.apply_command_edit_then(command, &mut capture, |_| {}));
    assert!(state
        .history
        .past
        .back()
        .unwrap()
        .scoped_snapshot()
        .is_some());
}

fn legacy_bytes(cache: &ChargeCache) -> usize {
    cache.nodes.retained_bytes()
        + cache.documents.retained_bytes()
        + cache.components.retained_bytes()
}

fn equivalent(state: &EditorState, cache: &mut ChargeCache) -> usize {
    let mut fallback = ChargeCache::default();
    let reference = resident_charge_mode(state, 1_000_000_000, &mut fallback, false).unwrap();
    let current = resident_charge_cached(state, 1_000_000_000, cache).unwrap();
    assert_eq!(
        current - cache.history.retained_bytes() - legacy_bytes(cache),
        reference - legacy_bytes(&fallback),
        "same owned allocation union, cache bytes separate"
    );
    current
}

#[test]
fn scoped_history_cached_and_fallback_ledgers_deduplicate_cloned_delta_owners() {
    let mut state = fixture();
    apply_scoped(&mut state, 1);
    let entry = state.history.past.back().unwrap().clone();
    assert!(entry.full_snapshot().is_none());
    let mut cache = ChargeCache::default();
    let one = equivalent(&state, &mut cache);
    state.history.past.push_back(entry.clone());
    let two = equivalent(&state, &mut cache);
    assert_eq!(
        two - one,
        std::mem::size_of_val(&entry),
        "cloned scoped entries share their Arc metadata, paths, roots and components"
    );
    assert_eq!(cache.history.additions, 0);
}

#[test]
fn mixed_full_and_scoped_history_undo_redo_releases_obsolete_weak_roots() {
    let mut state = fixture();
    state.history_push_past(state.snapshot_for_history());
    let mut cache = ChargeCache::default();
    for x in 1..=3 {
        apply_scoped(&mut state, x);
        equivalent(&state, &mut cache);
    }
    let scoped = state
        .history
        .past
        .back()
        .unwrap()
        .scoped_snapshot()
        .unwrap();
    let background = scoped.retained_roots().next().unwrap().clone();
    let weak_scope = Arc::downgrade(scoped);
    assert!(state.undo());
    equivalent(&state, &mut cache);
    assert!(state.redo());
    equivalent(&state, &mut cache);
    state.history.past.clear();
    state.history.future.clear();
    equivalent(&state, &mut cache);
    assert!(weak_scope.upgrade().is_none());
    assert_eq!(cache.history.retained_bytes(), 0);
    assert_eq!(Arc::weak_count(&background), 0);
}

#[test]
fn scoped_owned_storage_is_charged_and_optional_cache_falls_back_at_tight_limit() {
    let mut state = fixture();
    apply_scoped(&mut state, 1);
    let mut fallback = ChargeCache::default();
    let minimum = resident_charge_mode(&state, 1_000_000_000, &mut fallback, false).unwrap();
    let mut cache = ChargeCache::default();
    assert!(equivalent(&state, &mut cache) > minimum);
    assert_eq!(
        resident_charge_cached(&state, minimum, &mut cache).unwrap(),
        minimum
    );
    assert_eq!(cache.history.retained_bytes(), 0);
    assert!(matches!(
        resident_charge_cached(&state, minimum - 1, &mut cache),
        Err(ChargeError::OverLimit)
    ));
    state.history.past.clear();
    assert!(resident_charge_cached(&state, minimum - 1, &mut cache).is_ok());
}

#[test]
fn save_capture_skips_scoped_delta_and_reuses_a_real_complete_history_anchor() {
    let mut state = fixture();
    state.history_push_past(state.snapshot_for_history());
    apply_scoped(&mut state, 1);
    assert!(state.history.past.back().unwrap().full_snapshot().is_none());
    let full = state.history.past.front().unwrap().full_snapshot().unwrap();
    let saved = crate::doc_io::CanonicalSaveSnapshot::capture(&state);
    assert_eq!(saved.document().materialize(), state.doc);
    assert!(
        Arc::ptr_eq(saved.document().metadata_arc(), full.doc.metadata_arc()),
        "unchanged complete header reuses Full; no partial scoped forest is a save document"
    );
}
