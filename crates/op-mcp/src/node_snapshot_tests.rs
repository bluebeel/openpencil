use super::*;

// The established public shape serializes canonically then truncates. Keep
// that independent reference here while production avoids omitted subtrees.
fn reference(node: &PenNode, depth: i32) -> Value {
    fn truncate(value: &mut Value, depth: i32) {
        if let Some(Value::Array(items)) = value.get_mut("children") {
            if !items.is_empty() {
                if depth <= 0 {
                    *value.get_mut("children").unwrap() = Value::String("...".into());
                } else {
                    for child in items {
                        truncate(child, depth - 1);
                    }
                }
            }
        }
    }
    let mut v = serde_json::to_value(node).unwrap();
    if depth != -1 {
        truncate(&mut v, depth);
    }
    v
}

#[test]
fn depth_preserves_canonical_fields_for_every_node_kind() {
    for kind in [
        "frame",
        "group",
        "rectangle",
        "ellipse",
        "line",
        "polygon",
        "path",
        "text",
        "text_input",
        "image",
        "icon_font",
        "text_area",
        "select",
        "switch",
        "checkbox",
        "slider",
        "radio_group",
        "number_input",
        "progress",
        "tabs",
        "ref",
    ] {
        let source = serde_json::json!({"type":kind,"id":"n","name":"field parity",
            "x":1.25,"y":-4.5,"rotation":35,"visible":false,"opacity":0.5,
            "locked":true,"theme":{"Mode":"Dark"},"maskType":"alpha",
            "blendMode":"multiply","ref":"component","content":"typed text",
            "src":"https://example.com/image.png","d":"M0 0L1 1","polygonCount":5,"iconFontName":"heart",
            "tabs":[],"options":[],"children":[{"type":"frame","id":"inner",
                "children":[{"type":"text","id":"leaf","content":"nested"}]}]});
        let node: PenNode =
            serde_json::from_value(source).unwrap_or_else(|e| panic!("{kind}: {e}"));
        for depth in [-2, -1, 0, 1, 2, 5] {
            assert_eq!(
                value(&node, depth),
                reference(&node, depth),
                "{kind}, depth {depth}"
            );
        }
    }
}

#[test]
fn absent_empty_children_and_nested_metadata_keep_their_exact_shapes() {
    for source in [
        r#"{"type":"frame","id":"n"}"#,
        r#"{"type":"frame","id":"n","children":null}"#,
        r#"{"type":"frame","id":"n","children":[]}"#,
        r#"{"type":"ref","id":"n","ref":"c","descendants":{"label":{"children":[{"id":"override-child","content":"keep me"}]}},"children":[{"type":"text","id":"t","content":"hello"}]}"#,
    ] {
        let node: PenNode = serde_json::from_str(source).unwrap();
        for depth in [-1, 0, 1] {
            assert_eq!(value(&node, depth), reference(&node, depth));
        }
    }
}

#[test]
fn cutoff_skips_hidden_child_serialization() {
    struct MustNotSerialize;
    impl Serialize for MustNotSerialize {
        fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            panic!("omitted descendants were serialized");
        }
    }
    let node: PenNode = serde_json::from_str(
        r#"{"type":"frame","id":"n","children":[{"type":"text","id":"t","content":"hello"}]}"#,
    )
    .unwrap();
    let mut serializer = NodeSerializer {
        node: &node,
        depth: 0,
        fields: Map::new(),
        key: None,
    };
    ser::SerializeMap::serialize_entry(&mut serializer, "children", &MustNotSerialize).unwrap();
    assert_eq!(
        ser::SerializeMap::end(serializer).unwrap(),
        serde_json::json!({"children":"..."})
    );
}
