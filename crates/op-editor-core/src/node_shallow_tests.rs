use super::*;
use serde_json::json;

#[test]
fn shallow_clone_preserves_all_container_metadata_and_child_presence() {
    for kind in ["frame", "group", "rectangle", "tabs", "ref"] {
        for children in [
            None,
            Some(json!([])),
            Some(json!([{
                "id":"nested","type":"frame","children":[{"id":"leaf","type":"text","content":"Kept in source"}]
            }])),
        ] {
            let mut value = json!({"type":kind,"id":"root","name":"metadata","x":1.25,"y":2.5,
                "opacity":0.7,"rotation":13,"locked":true,"enabled":false,
                "state":{},"bindings":{},"events":{},"lifecycle":{},"semantics":{},"gestures":{}});
            if kind == "ref" {
                value["ref"] = json!("component");
                value["descendants"] = json!({"child":{"opacity":0.5,"content":"Override"}});
            } else {
                value["width"] = json!(120);
                value["height"] = json!(80);
                value["fill"] = json!([{"type":"solid","color":"#3366ff"}]);
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
            if let Some(children) = children {
                value["children"] = children;
            }
            let node: PenNode = serde_json::from_value(value).unwrap();
            let before = serde_json::to_value(&node).unwrap();
            let mut expected = before.clone();
            if expected.get("children").is_some() {
                expected["children"] = json!([]);
            }
            assert_eq!(
                serde_json::to_value(clone_without_children(&node)).unwrap(),
                expected,
                "{kind}"
            );
            assert_eq!(
                serde_json::to_value(&node).unwrap(),
                before,
                "source must remain unchanged"
            );
        }
    }
    let leaf: PenNode =
        serde_json::from_value(json!({"id":"text","type":"text","content":"Leaf","fontSize":24}))
            .unwrap();
    assert_eq!(clone_without_children(&leaf), leaf);
}
