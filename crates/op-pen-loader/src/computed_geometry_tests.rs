use super::*;
use op_editor_core::pen_node_ext::PenNodeExt;

fn fixture() -> EditorState {
    EditorState::from_document(jian_ops_schema::load_str(r##"{
     "version":"1.0.0","variables":{"brand":{"type":"color","value":"#3366ff"}},"pages":[
      {"id":"library","name":"L","children":[
       {"id":"master","type":"frame","reusable":true,"width":40,"height":30,"children":[
        {"id":"badge","type":"rectangle","x":3,"y":2,"width":6,"height":7}]}]},
      {"id":"p","name":"P","children":[
       {"id":"root","type":"frame","x":101.5,"y":51.25,"width":400,"height":"fit_content",
        "layout":"vertical","alignItems":"center","gap":8,"padding":[10,20],"rotation":13,
        "children":[
         {"id":"text","type":"text","content":"AV ffi measured","fontFamily":"Inter","fontSize":18,
          "width":"fit_content","height":"fit_content"},
         {"id":"row","type":"frame","layout":"horizontal","width":"fill_container","height":60,"gap":6,
          "children":[{"id":"flex","type":"rectangle","width":"fill_container","height":12},
           {"id":"instance","type":"ref","ref":"master","width":40,"height":30}]},
         {"id":"path","type":"path","d":"M0 0 L20 40 L40 0","width":40,"height":40},
         {"id":"line","type":"line","x":20,"y":10,"width":30,"height":0},
         {"id":"tabs","type":"tabs","width":100,"height":30,"children":[
          {"id":"panel","type":"frame","width":100,"height":60}]}]},
       {"id":"unbounded","type":"group","children":[
        {"id":"offset","type":"rectangle","x":-3,"y":-2,"width":5,"height":8}]},
       {"id":"open","type":"frame","width":5,"height":5,"clipContent":false,"children":[
        {"id":"escaped","type":"rectangle","x":40,"y":50,"width":10,"height":20}]},
       {"id":"closed","type":"frame","width":5,"height":5,"clipContent":true,"children":[
        {"id":"clipped","type":"rectangle","x":40,"y":50,"width":10,"height":20}]},
       {"id":"hidden","type":"rectangle","width":9000,"height":9000,"enabled":false}]}]
    }"##).unwrap().value)
}

#[test]
fn compact_computed_geometry_matches_every_full_scene_node_without_paint_storage() {
    fn equal(actual: &ComputedGeometry, expected: &SceneNode) {
        assert_eq!(actual.id, expected.id);
        assert_eq!(actual.kind, expected.kind);
        assert_eq!(
            actual.aggregate_bounds(),
            expected.aggregate_bounds(),
            "{}",
            actual.id
        );
        assert_eq!(
            actual.visual_bounds(),
            expected.visual_bounds(),
            "{} visual",
            actual.id
        );
        assert_eq!(actual.children.len(), expected.children.len());
        for (actual, expected) in actual.children.iter().zip(&expected.children) {
            equal(actual, expected);
        }
    }
    let mut state = fixture();
    state.ui.active_page_index = 1;
    for preserve in [false, true] {
        for workspace in [false, true] {
            state.editor_ui.preserve_authored_geometry = preserve;
            state.editor_ui.workspace.visible = workspace;
            for id in [None, Some("row"), Some("instance__badge"), Some("unknown")] {
                let full = crate::editor_state_to_page_item_layout_scene(&state, 1, id);
                let compact = editor_state_to_page_item_geometry(&state, 1, id);
                assert_eq!(compact.len(), full.pages[1].children.len());
                for (actual, expected) in compact.iter().zip(&full.pages[1].children) {
                    equal(actual, expected);
                }
            }
        }
    }
    assert!(std::mem::size_of::<ComputedGeometry>() < std::mem::size_of::<SceneNode>() / 4);
    println!(
        "representation bytes: node={} payload={} paint={} geometry={}",
        std::mem::size_of::<jian_ops_schema::node::PenNode>(),
        std::mem::size_of::<crate::payload::NodePayload>(),
        std::mem::size_of::<SceneNode>(),
        std::mem::size_of::<ComputedGeometry>()
    );
}

#[test]
fn flat_layout_storage_preserves_taffy_rects_and_duplicate_id_precedence() {
    use crate::adapter::pages::insert_flat_layout_subtree;
    use jian_core::{document::NodeTree, layout::LayoutEngine};
    let mut state = fixture();
    // Exercise explicit duplicate-ID precedence as well as nested/flex/text/tab layout.
    let duplicate = state.doc.pages.as_ref().unwrap()[1].children[0].clone();
    state.doc.pages.as_mut().unwrap()[1]
        .children
        .push(duplicate);
    for root in &state.doc.pages.as_ref().unwrap()[1].children {
        let mut legacy = NodeTree::new();
        legacy.insert_subtree(root.clone(), None);
        let mut flat = NodeTree::new();
        insert_flat_layout_subtree(&mut flat, root.clone(), None);
        assert_eq!(legacy.len(), flat.len());
        assert!(flat
            .nodes
            .values()
            .all(|data| data.schema.children().is_none_or(Vec::is_empty)));
        let measure = crate::adapter::layout_measure_backend();
        let mut before = LayoutEngine::with_backend(measure.clone());
        let mut after = LayoutEngine::with_backend(measure);
        let br = before.build(&legacy).unwrap();
        let ar = after.build(&flat).unwrap();
        let available = crate::root_available_size(root);
        before.compute(br[0], available).unwrap();
        after.compute(ar[0], available).unwrap();
        for (id, key) in &legacy.by_id {
            assert_eq!(
                before.node_rect(*key),
                after.node_rect(flat.by_id[id]),
                "{id}"
            );
        }
    }
}
