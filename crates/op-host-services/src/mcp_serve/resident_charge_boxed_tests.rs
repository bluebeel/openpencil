use super::*;
use op_editor_core::EditorCommand;
use serde_json::json;

fn state(events: bool, states: bool) -> EditorState {
    let mut node = json!({"type":"number_input","id":"root","width":40,"height":20});
    if events {
        node["events"] = json!({});
    }
    if states {
        node["states"] = json!({});
    }
    EditorState::from_document(
        serde_json::from_value(json!({"version":"1.0.0","children":[node]})).unwrap(),
    )
}

#[test]
fn present_empty_optional_boxes_are_charged_beyond_serialized_payload() {
    let plain = state(false, false);
    let base = resident_charge(&plain, 16 * 1024 * 1024).unwrap();
    for (events, states) in [(true, false), (false, true), (true, true)] {
        let boxed = state(events, states);
        let owned = boxed.doc.children[0].boxed_optional_storage_bytes();
        assert!(owned > 0, "present empty groups still own allocations");
        let charged = resident_charge(&boxed, 16 * 1024 * 1024).unwrap();
        assert!(
            charged >= base + owned * 2,
            "inline shrink must not hide owned storage: {charged}/{base}/{owned}"
        );
        assert!(matches!(
            resident_charge(&boxed, base + owned - 1),
            Err(ChargeError::OverLimit)
        ));
    }
}

#[test]
fn history_boxes_are_deduplicated_and_cached_accounting_matches_full_walk() {
    let mut editor = state(true, true);
    let checkpoint = editor.snapshot_for_history();
    for _ in 0..64 {
        editor.history_push_past(checkpoint.clone());
    }
    let mut cached = ChargeCache::default();
    let original = resident_charge_cached(&editor, 16 * 1024 * 1024, &mut cached).unwrap();
    assert_eq!(
        original,
        resident_charge(&editor, 16 * 1024 * 1024).unwrap()
    );
    assert!(editor.apply_undoable(EditorCommand::UpdateNode {
        node_id: op_editor_core::NodeId::new("root"),
        x: Some(5),
        y: None,
        width: None,
        height: None,
        name: None,
        fill_hex: None,
        page_id: None,
    }));
    let changed = resident_charge_cached(&editor, 16 * 1024 * 1024, &mut cached).unwrap();
    assert_eq!(changed, resident_charge(&editor, 16 * 1024 * 1024).unwrap());
    editor.history.past.clear();
    editor.history.future.clear();
    let cleared = resident_charge_cached(&editor, 16 * 1024 * 1024, &mut cached).unwrap();
    assert_eq!(cleared, resident_charge(&editor, 16 * 1024 * 1024).unwrap());
    assert!(cleared < changed);
}
