//! Finite numeric geometry updates leave unrelated large metadata untouched.
//! Unsupported keys/nulls use the complete canonical header serde path.
use crate::PenNodeExt;
use jian_ops_schema::{node::PenNode, sizing::SizingBehavior};
use serde_json::{Map, Value};

pub(crate) fn apply(node: &mut PenNode, patch: &Map<String, Value>) -> bool {
    if !patch.iter().all(|(key, value)| {
        matches!(key.as_str(), "x" | "y" | "rotation" | "width" | "height")
            && value.as_f64().is_some_and(f64::is_finite)
    }) {
        return false;
    }
    let (width, height) = dimension_presence(node);
    let base = node.base();
    let present = [
        base.x.is_some(),
        base.y.is_some(),
        base.rotation.is_some(),
        width,
        height,
    ];
    let mut bits = base.authored_geometry_nulls;
    for (index, present) in present.into_iter().enumerate() {
        if present {
            bits &= !(1 << index);
        }
    }
    if matches!(node, PenNode::Line(_) | PenNode::Ref(_)) {
        bits &= 0b111;
    }
    // Validation completed for every key before the first canonical write.
    for (key, value) in patch {
        let value = value.as_f64().unwrap();
        let index = match key.as_str() {
            "x" => {
                node.base_mut().x = Some(value);
                0
            }
            "y" => {
                node.base_mut().y = Some(value);
                1
            }
            "rotation" => {
                node.base_mut().rotation = Some(value);
                2
            }
            "width" => {
                set_dimension(node, true, value);
                3
            }
            "height" => {
                set_dimension(node, false, value);
                4
            }
            _ => unreachable!(),
        };
        bits &= !(1 << index);
    }
    node.base_mut().authored_geometry_nulls = bits;
    true
}
fn dimension_presence(node: &PenNode) -> (bool, bool) {
    match node {
        PenNode::Frame(n) => (n.container.width.is_some(), n.container.height.is_some()),
        PenNode::Group(n) => (n.container.width.is_some(), n.container.height.is_some()),
        PenNode::Rectangle(n) => (n.container.width.is_some(), n.container.height.is_some()),
        PenNode::Ellipse(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::Polygon(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::Path(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::Text(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::TextInput(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::Image(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::IconFont(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::TextArea(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::Select(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::Switch(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::Checkbox(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::Slider(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::RadioGroup(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::NumberInput(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::Progress(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::Tabs(n) => (n.width.is_some(), n.height.is_some()),
        PenNode::Line(_) | PenNode::Ref(_) => (false, false),
    }
}
fn set_dimension(node: &mut PenNode, width: bool, value: f64) {
    let value = Some(SizingBehavior::Number(value));
    macro_rules! assign {
        ($n:expr) => {
            if width {
                $n.width = value
            } else {
                $n.height = value
            }
        };
    }
    match node {
        PenNode::Frame(n) => assign!(n.container),
        PenNode::Group(n) => assign!(n.container),
        PenNode::Rectangle(n) => assign!(n.container),
        PenNode::Ellipse(n) => assign!(n),
        PenNode::Polygon(n) => assign!(n),
        PenNode::Path(n) => assign!(n),
        PenNode::Text(n) => assign!(n),
        PenNode::TextInput(n) => assign!(n),
        PenNode::Image(n) => assign!(n),
        PenNode::IconFont(n) => assign!(n),
        PenNode::TextArea(n) => assign!(n),
        PenNode::Select(n) => assign!(n),
        PenNode::Switch(n) => assign!(n),
        PenNode::Checkbox(n) => assign!(n),
        PenNode::Slider(n) => assign!(n),
        PenNode::RadioGroup(n) => assign!(n),
        PenNode::NumberInput(n) => assign!(n),
        PenNode::Progress(n) => assign!(n),
        PenNode::Tabs(n) => assign!(n),
        PenNode::Line(_) | PenNode::Ref(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_geometry_matches_canonical_serde_for_every_variant_and_null_shape() {
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
            for nulls in [false, true] {
                let mut value = serde_json::json!({"type":kind,"id":"root","name":"before","ref":"master","content":"Text","src":"https://example.com/x.png","d":"M0 0L1 1","polygonCount":5,"iconFontName":"heart","options":[],"tabs":[],"children":[{"type":"text","id":"child","content":"same","x":-0.0}]});
                if nulls {
                    for key in ["x", "y", "rotation", "width", "height"] {
                        value[key] = Value::Null;
                    }
                }
                let mut candidate: PenNode =
                    serde_json::from_value(value).unwrap_or_else(|error| panic!("{kind}:{error}"));
                let mut expected = serde_json::to_value(&candidate).unwrap();
                let patch = serde_json::json!({"x":11.899999618530273,"y":-0.0,"rotation":0.46000000834465027,"width":-0.0,"height":-3.25});
                for (key, value) in patch.as_object().unwrap() {
                    expected[key] = value.clone();
                }
                let expected: PenNode = serde_json::from_value(expected).unwrap();
                assert!(apply(&mut candidate, patch.as_object().unwrap()));
                assert_eq!(
                    serde_json::to_vec(&candidate).unwrap(),
                    serde_json::to_vec(&expected).unwrap(),
                    "{kind}:{nulls}"
                );
                assert_eq!(
                    crate::history_snapshot::metadata_sensitivity(&candidate),
                    crate::history_snapshot::metadata_sensitivity(&expected),
                    "{kind}:{nulls}"
                );
            }
        }
    }
    #[test]
    fn unsupported_mixed_patch_is_rejected_before_any_write() {
        let mut node: PenNode =
            serde_json::from_value(serde_json::json!({"type":"rectangle","id":"r","x":-0.0}))
                .unwrap();
        let before = serde_json::to_vec(&node).unwrap();
        for patch in [
            serde_json::json!({"x":7,"name":"new"}),
            serde_json::json!({"x":7,"width":null}),
            serde_json::json!({"x":7,"height":"fill_container"}),
        ] {
            assert!(!apply(&mut node, patch.as_object().unwrap()));
            assert_eq!(serde_json::to_vec(&node).unwrap(), before);
        }
    }
    #[test]
    fn geometry_edit_keeps_unrelated_large_string_allocation() {
        let content = "opaque".repeat(16384);
        let mut node: PenNode = serde_json::from_value(
            serde_json::json!({"type":"text","id":"t","content":content,"x":null}),
        )
        .unwrap();
        fn text(node: &PenNode) -> &str {
            let PenNode::Text(node) = node else { panic!() };
            let jian_ops_schema::node::TextContent::Plain(value) = &node.content else {
                panic!()
            };
            value
        }
        let ptr = text(&node).as_ptr();
        assert!(apply(
            &mut node,
            serde_json::json!({"x":17.125}).as_object().unwrap()
        ));
        assert_eq!(text(&node).as_ptr(), ptr);
        assert_eq!(text(&node), content);
    }
}
