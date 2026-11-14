use super::*;
use crate::test_support::{frame, rect, state_with};
use serde_json::json;

#[test]
fn recursive_snapshot_exact_canonical_node_bytes_and_child_presence() {
    for kind in [
        "frame",
        "group",
        "rectangle",
        "tabs",
        "ref",
        "text",
        "image",
        "path",
    ] {
        for children in [
            None,
            Some(json!([])),
            Some(
                json!([{"type":"text","id":"leaf","content":"Exact","fontSize":19,"fontFeatures":{"salt":1},"textDecorationSkipInk":false}]),
            ),
        ] {
            let mut value = json!({"type":kind,"id":"root","name":"Full metadata","x":1.25,"y":2.5,"rotation":13,"opacity":0.7,"locked":true,"enabled":false,"state":{},"bindings":{},"events":{},"lifecycle":{},"semantics":{},"gestures":{},"width":120,"height":80,"fill":[{"type":"solid","color":"#336699"}]});
            if kind == "ref" {
                value["ref"] = json!("component");
                value["descendants"] = json!({"child":{"opacity":0.5,"content":"Override"}});
            }
            if kind == "frame" {
                value["imageSearchQuery"] = json!("mountains");
                value["reusable"] = json!(true);
                value["slot"] = json!(["body"]);
                value["screen"] = json!("/home");
            }
            if kind == "tabs" {
                value["tabs"] = json!([{"value":"a","label":"A"}]);
                value["value"] = json!("a");
                value["states"] = json!({});
            }
            if kind == "text" {
                value["content"] = json!("Leaf");
            }
            if kind == "image" {
                value["src"] = json!("https://example.test/image.png");
            }
            if kind == "path" {
                value["path"] = json!("M0 0L1 1");
            }
            if let Some(children) = children {
                value["children"] = children;
            }
            let node: PenNode = serde_json::from_value(value).unwrap();
            let shared = SharedNode::capture(&node, None);
            assert_eq!(shared.materialize(), node);
            assert_eq!(shared.children().is_some(), node.children().is_some());
            assert_eq!(
                serde_json::to_vec(shared.as_ref()).unwrap(),
                serde_json::to_vec(&node).unwrap(),
                "{kind}: canonical field order must match too"
            );
        }
    }
}

#[test]
fn recursive_snapshot_shared_descendants_metadata_and_component_cow() {
    let leaves = vec![
        rect("a", "A", 0., 0., 8., 8.),
        rect("b", "B", 8., 0., 8., 8.),
    ];
    let mut state = state_with(vec![frame("root", "Root", 0., 0., 100., 100., leaves)]);
    let before = SharedDoc::capture(&state.doc, None);
    state.doc.children[0].children_mut().unwrap()[1]
        .base_mut()
        .x = Some(20.);
    let after = SharedDoc::capture(&state.doc, Some(&before));
    assert!(Arc::ptr_eq(before.metadata_arc(), after.metadata_arc()));
    let a = before.root_children()[0].children().unwrap();
    let b = after.root_children()[0].children().unwrap();
    assert!(Arc::ptr_eq(&a[0], &b[0]));
    assert!(!Arc::ptr_eq(&a[1], &b[1]));
    assert_eq!(
        after
            .snapshot_find_node(0, &NodeId::new("root"))
            .unwrap()
            .metadata()
            .children()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        after
            .snapshot_find_node(0, &NodeId::new("root"))
            .unwrap()
            .children()
            .unwrap()
            .len(),
        2,
        "normal lookup returns a shared view, never an owned subtree"
    );
    let mut repaired = after.clone();
    let replacement: PenNode =
        serde_json::from_value(json!({"type":"ref","id":"b","ref":"master"})).unwrap();
    repaired.repair_swap(&NodeId::new("b"), &replacement);
    assert!(Arc::ptr_eq(
        &after.root_children()[0].children().unwrap()[0],
        &repaired.root_children()[0].children().unwrap()[0]
    ));
    assert!(matches!(
        after
            .snapshot_find_node(0, &NodeId::new("b"))
            .unwrap()
            .metadata(),
        PenNode::Rectangle(_)
    ));
    assert!(matches!(
        repaired
            .snapshot_find_node(0, &NodeId::new("b"))
            .unwrap()
            .metadata(),
        PenNode::Ref(_)
    ));
    let mut lib = ComponentLibrary::default();
    lib.insert(Component {
        id: NodeId::new("root"),
        name: "Library".into(),
        root: before.root_children()[0].materialize(),
    });
    let original = SharedComponents::capture(&lib, None);
    lib.components[0].root = after.root_children()[0].materialize();
    let changed = SharedComponents::capture(&lib, Some(&original));
    assert!(Arc::ptr_eq(
        &original.components()[0].root.children().unwrap()[0],
        &changed.components()[0].root.children().unwrap()[0]
    ));
    assert_eq!(changed.materialize(), lib);
}

#[test]
fn recursive_snapshot_preserves_exact_streamed_image_table_order() {
    let a = format!("data:image/png;base64,{}", "A".repeat(512));
    let b = format!("data:image/png;base64,{}", "B".repeat(512));
    let value = json!({"version":"1.0.0","name":"Image order","children":[{"type":"frame","id":"root","fill":[{"type":"image","url":a}],"children":[{"type":"image","id":"a","src":a},{"type":"image","id":"b","src":b}]}],"pages":[{"id":"p","name":"Page","backgroundColor":"#fff","children":[{"type":"image","id":"c","src":b}]}]});
    let doc: PenDocument = serde_json::from_value(value).unwrap();
    let shared = SharedDoc::capture(&doc, None);
    let thumbnails = jian_ops_schema::image_thumbs::capture_snapshot();
    let (mut owned, mut recursive) = (Vec::new(), Vec::new());
    jian_ops_schema::image_table::write_document(&mut owned, &doc, &thumbnails).unwrap();
    jian_ops_schema::image_table::write_document(&mut recursive, &shared, &thumbnails).unwrap();
    assert_eq!(recursive, owned);
    assert_eq!(shared.materialize(), doc);
}

#[test]
#[ignore = "Run once under the shared1GB process guard; actual100 standalone undo/redo edits"]
fn recursive_history_100_standalone_edits_in_10000_node_frame() {
    use crate::EditorCommand;
    let leaves = (0..10_000)
        .map(|i| rect(&format!("leaf{i}"), "Leaf", 0., 0., 8., 8.))
        .collect();
    let mut state = state_with(vec![frame("root", "Root", 0., 0., 1000., 1000., leaves)]);
    let mut reference = state.doc.clone();
    for step in 1..=100 {
        assert!(state.apply_undoable(EditorCommand::UpdateNode {
            node_id: NodeId::new("leaf9999"),
            x: Some(step),
            y: None,
            width: None,
            height: None,
            name: None,
            fill_hex: None,
            page_id: None
        }));
        assert_eq!(state.history.past.len(), step as usize);
    }
    let first = &state.history.past.front().unwrap().doc.root_children()[0];
    let last = &state.history.past.back().unwrap().doc.root_children()[0];
    assert!(Arc::ptr_eq(
        &first.children().unwrap()[0],
        &last.children().unwrap()[0]
    ));
    for step in (0..100).rev() {
        assert!(state.undo());
        reference.children[0].children_mut().unwrap()[9999]
            .base_mut()
            .x = Some(step as f64);
        assert_eq!(
            serde_json::to_vec(&state.doc).unwrap(),
            serde_json::to_vec(&reference).unwrap()
        );
    }
    for step in 1..=100 {
        assert!(state.redo());
        reference.children[0].children_mut().unwrap()[9999]
            .base_mut()
            .x = Some(step as f64);
        assert_eq!(
            serde_json::to_vec(&state.doc).unwrap(),
            serde_json::to_vec(&reference).unwrap()
        );
    }
    let shared = SharedDoc::capture(&state.doc, Some(&state.history.past.back().unwrap().doc));
    assert_eq!(
        serde_json::to_vec(&shared).unwrap(),
        serde_json::to_vec(&state.doc).unwrap()
    );
    assert_eq!(state.history.past.len(), 100);
    assert!(state.history.future.is_empty());
}

#[test]
fn mutable_lookup_preserves_untouched_none_and_empty_child_presence() {
    let mut nodes: Vec<PenNode> = serde_json::from_value(json!([
        {"type":"rectangle","id":"absent"},
        {"type":"frame","id":"empty","children":[]},
        {"type":"frame","id":"nested","children":[{"type":"rectangle","id":"target","x":0}]}
    ]))
    .unwrap();
    let absent = serde_json::to_vec(&nodes[0]).unwrap();
    let empty = serde_json::to_vec(&nodes[1]).unwrap();
    crate::walkers::find_node_mut(&mut nodes, &NodeId::new("target"))
        .unwrap()
        .base_mut()
        .x = Some(3.);
    assert_eq!(serde_json::to_vec(&nodes[0]).unwrap(), absent);
    assert_eq!(serde_json::to_vec(&nodes[1]).unwrap(), empty);
    assert!(crate::walkers::find_node_mut(&mut nodes, &NodeId::new("missing")).is_none());
    assert!(nodes[0].children().is_none());
    assert_eq!(nodes[1].children().unwrap().len(), 0);
    assert!(
        nodes[0].children_mut().unwrap().is_empty(),
        "explicit insertion API still creates children"
    );
}

#[test]
fn unchanged_leaf_capture_keeps_exact_metadata_and_none_empty_transitions() {
    let mut node:PenNode=serde_json::from_value(json!({"type":"frame","id":"leaf","name":"Full metadata","opacity":0.35,"state":{},"bindings":{},"imageSearchQuery":"preserved","reusable":true,"fill":[{"type":"solid","color":"#123456"}]})).unwrap();
    let absent = SharedNode::capture(&node, None);
    let unchanged = SharedNode::capture(&node, Some(&absent));
    assert!(Arc::ptr_eq(&absent, &unchanged));
    node.children_mut().unwrap();
    let empty = SharedNode::capture(&node, Some(&absent));
    assert!(!Arc::ptr_eq(&absent, &empty));
    assert!(empty.children().unwrap().is_empty());
    let same_empty = SharedNode::capture(&node, Some(&empty));
    assert!(Arc::ptr_eq(&empty, &same_empty));
    if let PenNode::Frame(frame) = &mut node {
        frame.children = None
    } else {
        unreachable!()
    }
    let absent_again = SharedNode::capture(&node, Some(&empty));
    assert!(!Arc::ptr_eq(&absent_again, &empty));
    assert!(absent_again.children().is_none());
    assert_eq!(
        serde_json::to_vec(absent_again.as_ref()).unwrap(),
        serde_json::to_vec(&node).unwrap()
    );
    node.base_mut().opacity = Some(jian_ops_schema::node::NumberOrExpression::Number(0.6));
    let changed = SharedNode::capture(&node, Some(&absent_again));
    assert!(!Arc::ptr_eq(&changed, &absent_again));
    assert!(changed.equals_node(&node));
}

#[test]
fn sharing_preserves_signed_zero_and_json_object_wire_order() {
    let mut state = state_with(vec![rect("leaf", "Leaf", 0., 0., 8., 8.)]);
    let positive = SharedDoc::capture(&state.doc, None);
    state.doc.children[0].base_mut().x = Some(-0.0);
    assert_eq!(
        positive.root_children()[0].metadata(),
        &state.doc.children[0],
        "ordinary PartialEq cannot distinguish signed zero"
    );
    let negative = SharedDoc::capture(&state.doc, Some(&positive));
    assert!(!Arc::ptr_eq(
        &positive.root_children()[0],
        &negative.root_children()[0]
    ));
    assert_ne!(
        serde_json::to_vec(&positive).unwrap(),
        serde_json::to_vec(&negative).unwrap()
    );
    assert_eq!(
        serde_json::to_vec(&negative).unwrap(),
        serde_json::to_vec(&state.doc).unwrap()
    );
    let mut document: PenDocument = serde_json::from_value(
        json!({"version":"1.0.0","children":[],"state":{"count":{"type":"float","default":0.0}}}),
    )
    .unwrap();
    let first = SharedDoc::capture(&document, None);
    document
        .state
        .as_mut()
        .unwrap()
        .get_mut("count")
        .unwrap()
        .default = Some(json!(-0.0));
    let second = SharedDoc::capture(&document, Some(&first));
    assert!(!Arc::ptr_eq(first.metadata_arc(), second.metadata_arc()));
    assert_eq!(
        serde_json::to_vec(&second).unwrap(),
        serde_json::to_vec(&document).unwrap()
    );
    let mut node: PenNode = serde_json::from_value(
        json!({"type":"frame","id":"map","state":{"value":{"type":"object","default":{}}}}),
    )
    .unwrap();
    let mut a = serde_json::Map::new();
    a.insert("z".into(), json!(1));
    a.insert("a".into(), json!(2));
    if let PenNode::Frame(frame) = &mut node {
        frame
            .state
            .as_mut()
            .unwrap()
            .get_mut("value")
            .unwrap()
            .default = Some(serde_json::Value::Object(a));
    }
    let first = SharedNode::capture(&node, None);
    let mut b = serde_json::Map::new();
    b.insert("a".into(), json!(2));
    b.insert("z".into(), json!(1));
    if let PenNode::Frame(frame) = &mut node {
        frame
            .state
            .as_mut()
            .unwrap()
            .get_mut("value")
            .unwrap()
            .default = Some(serde_json::Value::Object(b));
    }
    assert_eq!(
        first.metadata(),
        &node,
        "JSON object equality deliberately ignores insertion order"
    );
    let second = SharedNode::capture(&node, Some(&first));
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(
        serde_json::to_vec(second.as_ref()).unwrap(),
        serde_json::to_vec(&node).unwrap()
    );
}

#[test]
fn metadata_probe_skips_large_image_values_and_restores_enclosing_save_scope() {
    let node:PenNode=serde_json::from_value(json!({"type":"image","id":"image","src":format!("data:image/png;base64,{}","A".repeat(1024*1024))})).unwrap();
    let thumbnails = jian_ops_schema::image_thumbs::ImageThumbSnapshot::default();
    jian_ops_schema::image_table::with_save_scope(&thumbnails, |tables| {
        let first = SharedNode::capture(&node, None);
        let second = SharedNode::capture(&node, Some(&first));
        assert!(Arc::ptr_eq(&first, &second));
        assert!(
            first.storage_bytes() < std::mem::size_of::<SharedNode>() + 4096,
            "sensitivity must not retain a second base64 image"
        );
        assert!(
            tables.images_is_empty(),
            "read-only capture must not register any source in enclosing save"
        );
        serde_json::to_writer(std::io::sink(), second.as_ref()).unwrap();
        assert!(
            !tables.images_is_empty(),
            "outer save scope must be restored"
        );
    });
}
