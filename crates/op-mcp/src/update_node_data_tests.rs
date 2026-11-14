//! TS-style `update_node(data)` parity tests.

use super::test_fixtures::sample;
use super::write_tools::update_node_snapshot;
use super::{EditorCommand, McpTool, ToolOutcome};
use op_editor_core::NodeId;
use serde_json::Value;
use std::collections::BTreeMap;

#[test]
fn update_node_data_with_ts_text_fields_emits_patch_command() {
    let mut args = BTreeMap::new();
    args.insert("nodeId".into(), "n11".into());
    args.insert(
        "data".into(),
        r##"{"content":"Updated","fontSize":24}"##.into(),
    );
    args.insert("pageId".into(), "page-2".into());

    let ToolOutcome::OkWithCommand(
        _,
        EditorCommand::PatchNodeData {
            node_id,
            patch_json,
            page_id,
        },
    ) = update_node_snapshot().call(&args)
    else {
        panic!("expected PatchNodeData command for TS text patch");
    };
    let patch: Value = serde_json::from_str(&patch_json).expect("patch json");
    assert_eq!(node_id.as_str(), "n11");
    assert_eq!(patch["content"], "Updated");
    assert_eq!(patch["fontSize"], 24);
    assert_eq!(page_id.as_deref(), Some("page-2"));
}

#[test]
fn update_node_data_with_sizing_keyword_emits_patch_command_and_applies() {
    let mut state = sample();
    let mut args = BTreeMap::new();
    args.insert("nodeId".into(), "n10".into());
    args.insert("data".into(), r##"{"height":"fit_content"}"##.into());

    let ToolOutcome::OkWithCommand(_, command @ EditorCommand::PatchNodeData { .. }) =
        update_node_snapshot().call(&args)
    else {
        panic!("expected PatchNodeData command for sizing keyword");
    };
    assert!(state.apply(command), "sizing keyword patch must apply");
    let node = op_editor_core::walkers::find_node(state.active_children(), &NodeId::new("n10"))
        .expect("updated frame");
    let value = serde_json::to_value(node).expect("updated node json");
    assert_eq!(value["height"], "fit_content");
}

#[test]
fn update_node_data_with_token_fill_emits_patch_command_and_preserves_token() {
    let mut state = sample();
    let mut args = BTreeMap::new();
    args.insert("nodeId".into(), "n10".into());
    args.insert(
        "data".into(),
        r##"{"fill":[{"type":"solid","color":"$--primary"}]}"##.into(),
    );

    let ToolOutcome::OkWithCommand(_, command @ EditorCommand::PatchNodeData { .. }) =
        update_node_snapshot().call(&args)
    else {
        panic!("expected PatchNodeData command for canonical token fill");
    };
    assert!(state.apply(command), "token fill patch must apply");
    let node = op_editor_core::walkers::find_node(state.active_children(), &NodeId::new("n10"))
        .expect("updated frame");
    let value = serde_json::to_value(node).expect("updated node json");
    assert_eq!(value["fill"][0]["color"], "$--primary");
}

#[test]
fn update_node_numeric_geometry_preserves_float_and_large_integer_bits() {
    for key in ["x", "y", "width", "height"] {
        for raw in ["11.899999618530273", "-0.0", "3.0", "2147483648", "1e30"] {
            let mut state = sample();
            let args = BTreeMap::from([
                ("nodeId".into(), "n10".into()),
                ("data".into(), format!(r#"{{"{key}":{raw}}}"#)),
            ]);
            let ToolOutcome::OkWithCommand(_, command @ EditorCommand::PatchNodeData { .. }) =
                update_node_snapshot().call(&args)
            else {
                panic!("{key}:{raw} must retain canonical geometry");
            };
            assert!(state.apply(command), "{key}:{raw} must apply");
            let node =
                op_editor_core::walkers::find_node(state.active_children(), &NodeId::new("n10"))
                    .unwrap();
            let actual = serde_json::to_value(node).unwrap()[key].as_f64().unwrap();
            assert_eq!(
                actual.to_bits(),
                raw.parse::<f64>().unwrap().to_bits(),
                "{key}:{raw}"
            );
        }
    }
    for raw in ["-2147483649", "18446744073709551615"] {
        let value: Value = serde_json::from_str(&format!(r#"{{"x":{raw}}}"#)).unwrap();
        let patch = super::update_node_data::ts_update_patch_json_value(&value)
            .unwrap()
            .expect("out-of-i32 integer must use canonical data");
        let actual: Value = serde_json::from_str(&patch).unwrap();
        assert_eq!(actual["x"], value["x"]);
    }
}

#[test]
fn update_node_i32_integer_literals_keep_the_lightweight_command() {
    for key in ["x", "y", "width", "height"] {
        for value in [i32::MIN, -1, 0, 3, i32::MAX] {
            let args = BTreeMap::from([
                ("nodeId".into(), "n10".into()),
                ("data".into(), format!(r#"{{"{key}":{value}}}"#)),
            ]);
            let outcome = update_node_snapshot().call(&args);
            if matches!(key, "width" | "height") && value < 0 {
                assert!(matches!(outcome, ToolOutcome::Err(_, _)));
                continue;
            }
            let ToolOutcome::OkWithCommand(
                _,
                EditorCommand::UpdateNode {
                    x,
                    y,
                    width,
                    height,
                    ..
                },
            ) = outcome
            else {
                panic!("{key}:{value} must keep the lightweight command");
            };
            let fields = [("x", x), ("y", y), ("width", width), ("height", height)];
            for (field, actual) in fields {
                assert_eq!(actual, (field == key).then_some(value), "{key}:{value}");
            }
        }
    }
}

#[test]
fn update_node_legacy_numeric_strings_do_not_change_routes() {
    for key in ["x", "y", "width", "height"] {
        for raw in ["11.899999618530273", "-0.0", "3.0", "2147483648", "17"] {
            let value = Value::Object(serde_json::Map::from_iter([(
                key.to_owned(),
                Value::String(raw.to_owned()),
            )]));
            assert!(super::update_node_data::ts_update_patch_json_value(&value)
                .unwrap()
                .is_none());
        }
    }
}
