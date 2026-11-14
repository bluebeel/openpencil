use super::*;
use serde_json::json;

fn reference(node: &PenNode, name: &str) -> Result<PenNode, serde_json::Error> {
    let mut value = serde_json::to_value(node)?;
    value["name"] = json!(name);
    serde_json::from_value(value)
}
#[test]
fn node_local_transform_matches_full_canonical_roundtrip_for_all_variants() {
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
        "tabs",
        "ref",
        "progress",
    ] {
        let node: PenNode = serde_json::from_value(json!({"type":kind,"id":"root","name":"before",
            "x":1.25,"y":-0.0,"width":50,"height":60,"rotation":12,"opacity":0.4,
            "fill":[{"type":"solid","color":"#123456"}],"state":{"a":{"type":"object","default":{"deep":[1,2,3]}}},
            "ref":"master","content":"Text","src":"https://example.com/x.png","d":"M0 0L1 1",
            "polygonCount":5,"iconFontName":"heart","options":[],"tabs":[],
            "children":[{"type":"frame","id":"child","children":[{"type":"ref","id":"nested","ref":"master"}]}]
        })).unwrap_or_else(|e| panic!("{kind}: {e}"));
        let candidate = map_subtree_json(&node, |value, root| {
            if root {
                value["name"] = json!("after")
            }
        })
        .unwrap();
        assert_eq!(
            serde_json::to_vec(&candidate).unwrap(),
            serde_json::to_vec(&reference(&node, "after").unwrap()).unwrap(),
            "{kind}"
        );
    }
}
#[test]
fn local_roundtrip_preserves_absent_empty_nested_children_and_type_changes() {
    for raw in [
        r#"{"type":"ref","id":"r","ref":"master"}"#,
        r#"{"type":"ref","id":"r","ref":"master","children":[]}"#,
        r#"{"type":"frame","id":"r","children":[{"type":"text","id":"t","content":"rich"}]}"#,
    ] {
        let node: PenNode = serde_json::from_str(raw).unwrap();
        for kind in ["frame", "ref", "rectangle", "ellipse"] {
            let mut value = serde_json::to_value(&node).unwrap();
            value["type"] = json!(kind);
            value["ref"] = json!("master");
            let expected = serde_json::from_value::<PenNode>(value);
            let actual = map_subtree_json(&node, |value, root| {
                if root {
                    value["type"] = json!(kind);
                    value["ref"] = json!("master")
                }
            });
            assert_eq!(actual.is_ok(), expected.is_ok());
            if let (Ok(actual), Ok(expected)) = (actual, expected) {
                assert_eq!(
                    serde_json::to_vec(&actual).unwrap(),
                    serde_json::to_vec(&expected).unwrap()
                );
            }
        }
    }
}
#[test]
fn deep_json_transform_keeps_complete_metadata_and_serde_failure_contract() {
    let mut node: PenNode =
        serde_json::from_value(json!({"type":"text","id":"leaf","content":"hello","fontSize":16}))
            .unwrap();
    for i in 0..32 {
        node =
            serde_json::from_value(json!({"type":"frame","id":format!("f{i}"),"children":[node]}))
                .unwrap();
    }
    let result = map_subtree_json(&node, |value, root| {
        if root {
            value["name"] = json!("mapped")
        }
    })
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&result).unwrap(),
        serde_json::to_vec(&reference(&node, "mapped").unwrap()).unwrap()
    );
    let actual = map_subtree_json(&node, |value, root| {
        if root {
            value["width"] = json!({"invalid":true})
        }
    });
    let mut bad = serde_json::to_value(&node).unwrap();
    bad["width"] = json!({"invalid":true});
    assert_eq!(
        actual.is_err(),
        serde_json::from_value::<PenNode>(bad).is_err()
    );
}

#[test]
fn leaf_transform_calls_once_and_preserves_absent_or_empty_children() {
    for raw in [
        r#"{"type":"ref","id":"leaf","ref":"master","name":"before"}"#,
        r#"{"type":"ref","id":"leaf","ref":"master","children":[],"name":"before"}"#,
        r#"{"type":"frame","id":"leaf","children":[],"name":"before"}"#,
        r#"{"type":"text","id":"leaf","content":"rich text","name":"before","fontSize":17.25}"#,
    ] {
        let source: PenNode = serde_json::from_str(raw).unwrap();
        let original = serde_json::to_vec(&source).unwrap();
        let mut calls = 0;
        let actual = map_subtree_json(&source, |value, root| {
            assert!(root);
            calls += 1;
            value["name"] = json!("after");
        })
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(
            serde_json::to_vec(&actual).unwrap(),
            serde_json::to_vec(&reference(&source, "after").unwrap()).unwrap()
        );
        assert_eq!(serde_json::to_vec(&source).unwrap(), original);
    }
}
