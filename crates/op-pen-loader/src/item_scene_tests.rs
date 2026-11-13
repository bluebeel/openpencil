use crate::{editor_state_to_item_layout_scene, editor_state_to_layout_scene};
use op_editor_core::EditorState;

fn state() -> EditorState {
    EditorState::from_document(
        jian_ops_schema::load_str(
            r##"{
      "version":"1.0.0","pages":[
        {"id":"library","name":"Library","children":[
          {"type":"frame","id":"master","reusable":true,"width":40,"height":30,
           "children":[{"type":"rectangle","id":"badge","width":20,"height":15,
           "fill":[{"type":"solid","color":"#ff0000"}]}]}]},
        {"id":"p","name":"P","children":[
          {"type":"rectangle","id":"unrelated","width":10,"height":10},
          {"type":"frame","id":"root","x":80,"y":60,"width":220,"height":100,
           "layout":"horizontal","gap":12,"rotation":13,"opacity":0.7,"children":[
            {"type":"rectangle","id":"before","width":50,"height":30},
            {"type":"ref","id":"instance","ref":"master","width":40,"height":30},
            {"type":"rectangle","id":"after","width":30,"height":30}]}]}]
    }"##,
        )
        .unwrap()
        .value,
    )
}

#[test]
fn item_scene_retains_whole_flex_root_global_components_and_page_metadata() {
    for preserve in [false, true] {
        let mut state = state();
        state.editor_ui.preserve_authored_geometry = preserve;
        let full = editor_state_to_layout_scene(&state);
        let item = editor_state_to_item_layout_scene(&state, "instance");
        assert_eq!(item.active_page_index, full.active_page_index);
        assert_eq!(item.pages.len(), full.pages.len());
        assert_eq!(item.pages[0].id, "library");
        assert!(item.pages[0].children.is_empty());
        assert_eq!(item.pages[1].name, "P");
        assert_eq!(
            item.pages[1].children,
            vec![full.pages[1].children[1].clone()]
        );
        let root = &item.pages[1].children[0];
        assert_eq!(root.children.len(), 3);
        assert!(root.children[1].children[0].fill.is_some());
        assert_eq!(item.pages[1].find("after"), full.pages[1].find("after"));
    }
}

#[test]
fn page_request_retains_all_page_roots_and_virtual_ids_fall_back_to_full_scene() {
    let state = state();
    let full = editor_state_to_layout_scene(&state);
    let page = editor_state_to_item_layout_scene(&state, "p");
    assert_eq!(page.pages[1], full.pages[1]);
    assert!(page.pages[0].children.is_empty());
    for id in ["instance__badge", "unknown"] {
        let fallback = editor_state_to_item_layout_scene(&state, id);
        assert_eq!(fallback.pages, full.pages);
    }
}

#[test]
fn item_scene_keeps_presentation_contract_and_implicit_page_identity() {
    let mut state = state();
    state.editor_ui.workspace.visible = true;
    assert_eq!(
        editor_state_to_item_layout_scene(&state, "instance").pages,
        editor_state_to_layout_scene(&state).pages
    );
    let state = EditorState::from_document(
        jian_ops_schema::load_str(
            r#"{
        "version":"1.0.0","name":"Untitled","children":[
        {"id":"a","type":"rectangle","width":10,"height":10},
        {"id":"b","type":"rectangle","width":10,"height":10}] }"#,
        )
        .unwrap()
        .value,
    );
    let item = editor_state_to_item_layout_scene(&state, "b");
    assert_eq!(item.pages[0].id, "page-1");
    assert_eq!(item.pages[0].name, "Untitled");
    assert_eq!(item.pages[0].children.len(), 1);
    assert_eq!(item.pages[0].children[0].id, "b");
}
