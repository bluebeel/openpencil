use super::*;
use op_editor_core::{command::LayoutPropValue, EditorCommand, EditorState, NodeId};

fn state(preserve: bool) -> EditorState {
    let doc = jian_ops_schema::load_str(r#"{"version":"1.0.0","children":[
        {"id":"old","type":"frame","x":700.25,"y":-0.0,"width":200,"height":120,
         "rotation":0.000001,"opacity":0.6000000238418579,"layout":"horizontal","gap":999,
         "children":[{"id":"old-a","type":"rectangle","x":37,"y":45,"width":16,"height":12},
                     {"id":"old-b","type":"frame","x":91,"y":2,"width":16,"height":12,"children":[]}]},
        {"id":"new","type":"frame","x":40,"y":60,"width":200,"height":120,
         "children":[{"id":"a","type":"rectangle","x":0,"y":0,"width":16,"height":12},
                     {"id":"b","type":"rectangle","x":0,"y":0,"width":16,"height":12},
                     {"id":"c","type":"rectangle","x":0,"y":0,"width":16,"height":12}]}]}"#).unwrap().value;
    let mut state = EditorState::from_document(doc);
    state.editor_ui.preserve_authored_geometry = preserve;
    state
}
fn patch(state: &mut EditorState, json: &str) {
    assert!(state.apply_undoable(EditorCommand::PatchNodeData {
        node_id: NodeId::new("new"),
        patch_json: json.to_owned(),
        page_id: None,
    }));
}
fn bounds(state: &EditorState, id: &str) -> [f32; 4] {
    let geometry = crate::editor_state_to_page_item_geometry(state, 0, Some("new"));
    let rect = geometry
        .iter()
        .find_map(|root| root.find(id))
        .unwrap()
        .aggregate_bounds();
    [
        rect.origin.x,
        rect.origin.y,
        rect.size.x,
        rect.size.y,
    ]
}

#[test]
fn explicit_layout_edits_reflow_in_both_modes_without_rewriting_imported_nodes() {
    for preserve in [true, false] {
        let mut state = state(preserve);
        let old = serde_json::to_vec(&state.doc.children[0]).unwrap();
        let children = serde_json::to_value(&state.doc.children[1]).unwrap()["children"].clone();
        patch(
            &mut state,
            r#"{"layout":"horizontal","gap":10,"padding":8,"justifyContent":"start","alignItems":"start"}"#,
        );
        assert_eq!(bounds(&state, "a"), [48.0, 68.0, 16.0, 12.0]);
        assert_eq!(bounds(&state, "b"), [74.0, 68.0, 16.0, 12.0]);
        assert_eq!(bounds(&state, "c"), [100.0, 68.0, 16.0, 12.0]);
        patch(&mut state, r#"{"layout":"vertical","gap":6}"#);
        assert_eq!(bounds(&state, "a"), [48.0, 68.0, 16.0, 12.0]);
        assert_eq!(bounds(&state, "b"), [48.0, 86.0, 16.0, 12.0]);
        assert_eq!(bounds(&state, "c"), [48.0, 104.0, 16.0, 12.0]);
        assert_eq!(serde_json::to_vec(&state.doc.children[0]).unwrap(), old);
        assert_eq!(
            serde_json::to_value(&state.doc.children[1]).unwrap()["children"],
            children
        );
        assert_eq!(state.editor_ui.preserve_authored_geometry, preserve);
    }
}

#[test]
fn computed_island_keeps_imported_grandchildren_and_explicit_absolute_constraints() {
    let doc = jian_ops_schema::load_str(
        r#"{"version":"1.0.0","children":[
      {"id":"root","type":"frame","x":100,"y":200,"width":300,"height":120,
       "layout":"horizontal","preserveGeometry":false,"gap":10,"padding":8,"children":[
        {"id":"nested","type":"frame","x":17,"y":23,"width":60,"height":40,
         "layout":"horizontal","gap":999,"padding":999,"children":[
          {"id":"inner","type":"rectangle","x":7,"y":9,"width":10,"height":11}]},
        {"id":"flow","type":"rectangle","x":0,"y":0,"width":16,"height":12},
        {"id":"absolute","type":"rectangle","x":140,"y":50,"width":16,"height":12,
         "constraints":{"h":"left","v":"top"}}]}]}"#,
    )
    .unwrap()
    .value;
    let before = serde_json::to_vec(&doc).unwrap();
    let rects = rects_for_roots(&doc.children);
    assert_eq!(rects["nested"], [108.0, 208.0, 60.0, 40.0]);
    assert_eq!(rects["inner"], [115.0, 217.0, 10.0, 11.0]);
    assert_eq!(rects["flow"], [178.0, 208.0, 16.0, 12.0]);
    assert_eq!(rects["absolute"][0], 240.0);
    assert_eq!(rects["absolute"][1], 250.0);
    assert_eq!(serde_json::to_vec(&doc).unwrap(), before);
}

#[test]
fn local_override_is_typed_persistent_and_restored_exactly_by_undo_redo() {
    let mut state = state(true);
    let before = serde_json::to_vec(&state.doc).unwrap();
    assert!(state.apply_undoable(EditorCommand::SetNodeLayoutProp {
        node_id: NodeId::new("new"),
        property: "layout".to_owned(),
        value: LayoutPropValue::Keyword("horizontal".to_owned()),
    }));
    assert!(state.editor_ui.preserve_authored_geometry);
    let after = serde_json::to_vec(&state.doc).unwrap();
    assert_eq!(
        serde_json::to_value(&state.doc.children[1]).unwrap()["preserveGeometry"],
        false
    );
    let restored = jian_ops_schema::load_str(std::str::from_utf8(&after).unwrap())
        .unwrap()
        .value;
    let mut reopened = EditorState::from_document(restored);
    reopened.editor_ui.preserve_authored_geometry = true;
    for id in ["a", "b", "c"] {
        assert_eq!(bounds(&state, id), bounds(&reopened, id));
    }
    assert!(state.apply(EditorCommand::Undo));
    assert_eq!(serde_json::to_vec(&state.doc).unwrap(), before);
    assert!(state.editor_ui.preserve_authored_geometry);
    assert!(state.apply(EditorCommand::Redo));
    assert_eq!(serde_json::to_vec(&state.doc).unwrap(), after);
}
