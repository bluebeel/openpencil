//! Nested grouping preserves the established flat operation's exact records.

use super::*;
use crate::{EditorCommand, SelectionState};
use serde_json::json;

fn id(value: &str) -> NodeId {
    NodeId::new(value)
}
fn document(nested: bool) -> jian_ops_schema::PenDocument {
    let members = json!([
        {"type":"rectangle","id":"a","x":-0.0,"y":0.46000000834465027,
         "width":11.899999618530273,"height":8,"rotation":27.125,"flipX":true,
         "events":{"onTap":[{"set":{"$state.x":-0.0}}]}},
        {"type":"text","id":"b","x":31.25,"y":15.833333015441895,"width":48,
         "height":18,"content":"quoted \" Ω","fontFeatures":{"kern":0,"liga":1},
         "textDecorationSkipInk":false},
        {"type":"frame","id":"c","children":[]}
    ]);
    let mut roots = json!([
        {"type":"frame","id":"outside-absent","name":"Keep absent"},
        {"type":"frame","id":"outside-empty","children":[]}
    ])
    .as_array()
    .unwrap()
    .clone();
    if nested {
        roots.push(json!({"type":"frame","id":"parent","x":209.52000427246094,
            "y":-0.0,"rotation":-15.25,"flipY":true,"children":members}));
    } else {
        roots.extend(members.as_array().unwrap().clone());
    }
    serde_json::from_value(
        json!({"version":"1.0.0","name":"All metadata retained","children":roots}),
    )
    .unwrap()
}
fn bytes(state: &EditorState) -> Vec<u8> {
    serde_json::to_vec(&state.doc).unwrap()
}
fn members(state: &EditorState, nested: bool) -> &[PenNode] {
    if nested {
        state.doc.children.last().unwrap().children().unwrap()
    } else {
        &state.doc.children[2..]
    }
}
fn select(state: &mut EditorState, ids: &[&str]) {
    assert!(state.apply(EditorCommand::SetSelectionSet {
        node_ids: ids.iter().map(|value| id(value)).collect(),
    }));
}

#[test]
fn nested_and_flat_group_round_trip_keep_sibling_order_and_complete_child_bits() {
    for nested in [false, true] {
        let mut state = EditorState::from_document(document(nested));
        select(&mut state, &["b", "a"]);
        let before = bytes(&state);
        let child_bytes: Vec<_> = members(&state, nested)[..2]
            .iter()
            .map(|node| serde_json::to_vec(node).unwrap())
            .collect();
        let mut next = 1;
        let grouped = state.group_selected(&mut next).unwrap();
        let siblings = members(&state, nested);
        assert_eq!(siblings.len(), 2);
        assert_eq!(siblings[0].id_str(), grouped.as_str());
        assert_eq!(siblings[1].id_str(), "c");
        for (child, expected) in siblings[0].children().unwrap().iter().zip(&child_bytes) {
            assert_eq!(&serde_json::to_vec(child).unwrap(), expected);
        }
        assert_eq!(state.selection.set, vec![grouped.clone()]);
        assert_eq!(state.selection.anchor, grouped);
        assert!(state.ungroup_selected());
        assert_eq!(bytes(&state), before);
        assert_eq!(state.selection.set, vec![id("a"), id("b")]);
        assert_eq!(state.selection.anchor, id("b"));
    }
}

#[test]
fn nested_group_undo_redo_is_exact_under_transformed_parent() {
    let mut state = EditorState::from_document(document(true));
    select(&mut state, &["a", "b"]);
    let before = bytes(&state);
    assert!(state.apply_undoable(EditorCommand::GroupSelected));
    let grouped = bytes(&state);
    assert!(state.apply_undoable(EditorCommand::UngroupSelected));
    assert_eq!(bytes(&state), before);
    assert_eq!(state.history.past.len(), 2);
    assert!(state.apply(EditorCommand::Undo));
    assert_eq!(bytes(&state), grouped);
    assert!(state.apply(EditorCommand::Undo));
    assert_eq!(bytes(&state), before);
    assert!(state.apply(EditorCommand::Redo));
    assert_eq!(bytes(&state), grouped);
    assert!(state.apply(EditorCommand::Redo));
    assert_eq!(bytes(&state), before);
}

#[test]
fn invalid_or_mixed_parent_selection_rejects_before_id_or_document_changes() {
    for chosen in [
        ["a", "outside-empty"],
        ["a", "missing"],
        ["a", "a"],
        ["parent", "a"],
    ] {
        let mut state = EditorState::from_document(document(true));
        // Public mutation is deliberately exercised, including duplicate/unknown IDs.
        state.selection = SelectionState {
            set: chosen.iter().map(|value| id(value)).collect(),
            anchor: id(chosen[1]),
        };
        let before = bytes(&state);
        let mut next = 50;
        assert!(state.group_selected(&mut next).is_none());
        assert_eq!(next, 50);
        assert_eq!(bytes(&state), before);
    }
}

#[test]
fn structural_parent_path_does_not_alias_duplicate_parent_ids() {
    let doc = serde_json::from_value(json!({"version":"1.0.0","children":[
        {"type":"frame","id":"duplicate","children":[{"type":"rectangle","id":"unrelated"}]},
        {"type":"frame","id":"duplicate","children":[
            {"type":"rectangle","id":"a"},{"type":"rectangle","id":"b"}]}
    ]}))
    .unwrap();
    let mut state = EditorState::from_document(doc);
    select(&mut state, &["a", "b"]);
    let untouched = serde_json::to_vec(&state.doc.children[0]).unwrap();
    let before = bytes(&state);
    assert!(state.group_selected(&mut 1).is_some());
    assert_eq!(
        serde_json::to_vec(&state.doc.children[0]).unwrap(),
        untouched
    );
    assert_eq!(state.doc.children[1].children().unwrap().len(), 1);
    assert!(state.ungroup_selected());
    assert_eq!(bytes(&state), before);
}

#[test]
fn nested_empty_group_removal_preserves_unrelated_absent_and_empty_arrays() {
    for explicit in [false, true] {
        let mut group = json!({"type":"group","id":"empty-group"});
        if explicit {
            group["children"] = json!([]);
        }
        let doc = serde_json::from_value(json!({"version":"1.0.0","children":[
            {"type":"frame","id":"absent"}, {"type":"frame","id":"empty","children":[]},
            {"type":"frame","id":"parent","children":[group]}
        ]}))
        .unwrap();
        let mut state = EditorState::from_document(doc);
        select(&mut state, &["empty-group"]);
        let unaffected = serde_json::to_vec(&state.doc.children[..2]).unwrap();
        assert!(state.ungroup_selected());
        assert_eq!(
            serde_json::to_vec(&state.doc.children[..2]).unwrap(),
            unaffected
        );
        assert!(state.doc.children[2].children().unwrap().is_empty());
        assert!(state.selection.set.is_empty());
        assert!(!state.selection.anchor.is_real());
    }
}
