//! Full-scene equivalence for the request-scoped export path.
use super::*;
use op_editor_core::NodeId;

fn fixture() -> EditorState {
    let mut src: serde_json::Value = serde_json::from_str(
        r##"{
      "version":"1.0.0",
      "variables":{"brand":{"type":"color","value":"#3366ff"}},
      "pages":[
        {"id":"library","name":"Library","children":[
          {"type":"frame","id":"master","reusable":true,"width":40,"height":30,
           "fill":[{"type":"solid","color":"$brand"}],
           "children":[{"type":"rectangle","id":"badge","x":3,"y":4,
             "width":12,"height":12,"fill":[{"type":"solid","color":"#ff8800"}]}]}]},
        {"id":"p","name":"P","children":[
          {"type":"rectangle","id":"unrelated","width":10,"height":10},
          {"type":"frame","id":"root","x":50,"y":60,"width":220,"height":130,
           "rotation":13,"opacity":0.7,"layout":"horizontal","gap":12,
           "fill":[{"type":"solid","color":"#eeeeee"}],
           "effects":[{"type":"shadow","offsetX":2,"offsetY":3,
                        "blur":4,"spread":1,"color":"#00000080"}],
           "children":[
            {"type":"frame","id":"column","width":70,"height":100,
             "layout":"vertical","gap":6,"clipContent":true,"children":[
              {"type":"rectangle","id":"first","width":30,"height":20},
              {"type":"rectangle","id":"photo","width":40,"height":30,
               "fill":[{"type":"image","url":"IMAGE_URL","mode":"crop",
                        "originalSize":{"width":4,"height":1},
                        "transform":{"m00":0.5,"m01":0,"m02":0.25,
                                     "m10":0,"m11":1,"m12":0}}]}]},
            {"type":"ref","id":"instance","ref":"master","width":40,"height":30}]}]}]
    }"##,
    )
    .unwrap();
    src["pages"][1]["children"][1]["children"][0]["children"][1]["fill"][0]["url"] =
        serde_json::Value::String("data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAQAAAABCAYAAAD5PA/NAAAAEUlEQVR4nGP4z8DwH4Sh1H8AP9EH+YU+c5sAAAAASUVORK5CYII=".into());
    EditorState::from_document(jian_ops_schema::load_str(&src.to_string()).unwrap().value)
}

#[test]
fn requested_item_scene_matches_full_pixels_for_nested_flex_component_shadow_and_crop() {
    for preserve in [false, true] {
        let mut state = fixture();
        state.editor_ui.preserve_authored_geometry = preserve;
        for id in [
            "root",
            "column",
            "photo",
            "instance",
            "instance__badge",
            "p",
        ] {
            let args = BTreeMap::from([
                ("itemId".into(), id.into()),
                ("format".into(), "png".into()),
            ]);
            let full = export_item_snapshot(&state);
            let focused = export_item_snapshot_for_call(&state, Some(&args));
            let full_page = full.scene.pages.iter().find(|p| p.id == "p").unwrap();
            let focused_page = focused.scene.pages.iter().find(|p| p.id == "p").unwrap();
            assert_eq!(focused_page.find("root"), full_page.find("root"), "{id}");
            let root = full_page.find("root").unwrap();
            assert!(
                !root.effects.is_empty(),
                "shadow fixture must reach the renderer"
            );
            assert!((root.composite_opacity - 0.7).abs() < 0.001);
            let photo = full_page.find("photo").unwrap();
            assert!(photo.image_src.is_some() && photo.image_transform.is_some());
            assert_eq!(full_page.find("instance").unwrap().children.len(), 1);
            let bytes = |outcome| match outcome {
                ToolOutcome::OkJson(json) => {
                    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
                    base64::engine::general_purpose::STANDARD
                        .decode(json["bytes_base64"].as_str().unwrap())
                        .unwrap()
                }
                outcome => panic!("{id}: {outcome:?}"),
            };
            let expected = bytes(full.call(&args));
            assert!(expected.len() > 80);
            assert_eq!(
                bytes(focused.call(&args)),
                expected,
                "{id}, preserve={preserve}"
            );
        }
    }
}

#[test]
fn request_without_item_id_uses_the_captured_selection() {
    let mut state = fixture();
    state.selection.anchor = NodeId::new("photo");
    let args = BTreeMap::from([("format".into(), "png".into())]);
    let tool = export_item_snapshot_for_call(&state, Some(&args));
    assert!(tool.scene.pages[0].children.is_empty());
    assert_eq!(tool.scene.pages[1].children.len(), 1);
    assert!(matches!(tool.call(&args), ToolOutcome::OkJson(_)));
}

#[test]
fn requested_page_geometry_builder_matches_full_nested_flex_component_shadow_crop_pixels() {
    for preserve in [false, true] {
        let mut state = fixture();
        state.editor_ui.preserve_authored_geometry = preserve;
        let mut full = export_item_snapshot(&state);
        let scene = op_pen_loader::editor_state_to_page_item_layout_scene(&state, 1, Some("photo"));
        assert!(scene.pages[0].children.is_empty());
        let root = full.scene.pages[1].find("root").unwrap();
        assert_eq!(scene.pages[1].find("root"), Some(root));
        let args = BTreeMap::from([
            ("itemId".into(), "root".into()),
            ("format".into(), "png".into()),
        ]);
        let expected = full.call(&args);
        full.scene = RasterSnapshot::from_scene(scene);
        assert_eq!(full.call(&args), expected, "preserve={preserve}");
    }
}
