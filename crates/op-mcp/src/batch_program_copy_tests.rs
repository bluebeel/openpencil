//! Copy conversions remain equivalent to the previous complete JSON path.
use crate::batch_program_test_support::{binding_id, call_operations};
use op_editor_core::{EditorState, PenNodeExt};
use serde_json::{json, Value};

fn old_copy(
    source: &jian_ops_schema::node::PenNode,
    overrides: &Value,
) -> jian_ops_schema::node::PenNode {
    let mut value = serde_json::to_value(source).unwrap();
    for (key, value_override) in overrides.as_object().unwrap() {
        if key != "id" && key != "descendants" {
            value[key] = value_override.clone();
        }
    }
    crate::batch_design::normalize_node_shape(&mut value);
    serde_json::from_value(value).unwrap()
}
fn erase_ids(value: &mut Value) {
    value.as_object_mut().unwrap().remove("id");
    if let Some(children) = value.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            erase_ids(child);
        }
    }
}
#[test]
fn batch_program_copy_matches_recursive_normalization_and_explicit_children_fallback() {
    let document: jian_ops_schema::PenDocument = serde_json::from_value(json!({"version":"1.0.0","children":[
        {"type":"frame","id":"source","name":"Original","children":[
            {"type":"text","id":"text","content":"fit default"},
            {"type":"ref","id":"inherited","ref":"master"},
            {"type":"ref","id":"empty","ref":"master","children":[]},
            {"type":"rectangle","id":"solid","width":20,"height":30,"fill":[{"type":"solid","color":"#123456"}]}
        ]}
    ]})).unwrap();
    for overrides in [
        json!({}),
        json!({"name":"Copied","id":"ignored","descendants":{"text":{"content":"ignored"}}}),
        json!({"type":"group","opacity":0.4}),
        json!({"children":[{"type":"text","id":"new","content":"replacement"}]}),
    ] {
        let mut state = EditorState::from_document(document.clone());
        let expected = old_copy(&state.doc.children[0], &overrides);
        let (envelope, command) = call_operations(
            &state,
            &format!(
                "copy=C(\"source\", null, {overrides})\nU(\"source\", {{\"name\":\"Original\"}})"
            ),
        );
        assert!(envelope.get("errors").is_none(), "{envelope}");
        assert!(state.apply(command.unwrap()));
        let id = binding_id(&envelope, "copy");
        let actual = op_editor_core::walkers::find_node(
            state.active_children(),
            &op_editor_core::NodeId::new(id),
        )
        .unwrap();
        let mut a = serde_json::to_value(actual).unwrap();
        let mut b = serde_json::to_value(expected).unwrap();
        erase_ids(&mut a);
        erase_ids(&mut b);
        assert_eq!(a, b);
        assert_eq!(state.doc.children[0].id_str(), "source");
    }
}
#[test]
fn batch_program_copy_id_proof_keeps_freed_ids_aliases_and_transactional_failure() {
    let mut state = EditorState::from_document(
        serde_json::from_value(json!({"version":"1.0.0","children":[
        {"type":"frame","id":"source","children":[{"type":"text","id":"child","content":"A"}]}]}))
        .unwrap(),
    );
    let (envelope, command) = call_operations(
        &state,
        "a=C(\"source\",null)\nD(a)\nb=C(\"source\",null)\nU(\"source/child\",{\"content\":\"B\"})",
    );
    assert!(envelope.get("errors").is_none(), "{envelope}");
    assert_ne!(binding_id(&envelope, "a"), binding_id(&envelope, "b"));
    assert!(state.apply(command.unwrap()));
    assert_eq!(
        serde_json::to_value(&state.doc.children[0]).unwrap()["children"][0]["content"],
        "B"
    );
    let before = serde_json::to_vec(&state.doc).unwrap();
    let (failed, command) =
        call_operations(&state, "a=C(\"source\",null)\nU(a,{\"children\":false})");
    assert_eq!(failed["applied"], false);
    assert!(command.is_none());
    assert_eq!(serde_json::to_vec(&state.doc).unwrap(), before);
}
