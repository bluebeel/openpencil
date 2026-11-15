//! Request-specific reads preserve the legacy query contract and snapshot time.

use std::collections::BTreeMap;

use op_editor_core::{EditorState, PenNodeExt};
use serde_json::{json, Value};

use crate::{
    batch_get_snapshot, batch_get_snapshot_for_call, read_nodes_snapshot,
    read_nodes_snapshot_for_call, McpTool, ToolOutcome,
};

fn fixture() -> EditorState {
    EditorState::from_document(jian_ops_schema::load_str(r##"{
      "version":"1.0.0","variables":{"brand":{"type":"color","value":"#3366ff"}},
      "pages":[{"id":"first","name":"First","children":[
        {"type":"frame","id":"one","width":100,"height":100,"children":[
          {"type":"rectangle","id":"dup","name":"First match","width":10,"height":10}]},
        {"type":"frame","id":"two","width":100,"height":100,"children":[
          {"type":"rectangle","id":"dup","name":"Second match","width":10,"height":10}]},
        {"type":"rectangle","id":"token","width":10,"height":10,"fill":[{"type":"solid","color":"$brand"}]}]},
        {"id":"other","name":"Other","children":[{"type":"rectangle","id":"elsewhere","width":10,"height":10}]}]
    }"##).unwrap().value)
}

fn args(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn value(outcome: ToolOutcome) -> Value {
    match outcome {
        ToolOutcome::OkJson(json) => serde_json::from_str(&json).unwrap(),
        outcome => panic!("unexpected outcome: {outcome:?}"),
    }
}

#[test]
fn request_read_nodes_preserves_page_depth_aliases_duplicate_lookup_and_errors() {
    let state = fixture();
    for arguments in [
        args(&[]),
        args(&[
            ("nodeIds", "[\"two\",\"dup\",\"dup\",\"missing\"]"),
            ("depth", "0"),
        ]),
        args(&[
            ("node_ids", "dup,token"),
            ("page_id", "first"),
            ("include_variables", "true"),
        ]),
        args(&[("page", "other"), ("ids", "elsewhere"), ("depth", "1")]),
        args(&[("page", "missing"), ("depth", "invalid")]),
        args(&[("depth", "invalid")]),
        args(&[("includeVariables", "invalid"), ("nodeIds", "[1]")]),
        args(&[("nodeIds", "[1]")]),
        args(&[("nodeIds", "[]"), ("depth", "0")]),
    ] {
        assert_eq!(
            read_nodes_snapshot_for_call(&state, &arguments).call(&arguments),
            read_nodes_snapshot(&state).call(&arguments),
            "{arguments:?}"
        );
    }
    let arguments = args(&[("nodeIds", "[\"two\",\"dup\",\"dup\"]"), ("depth", "0")]);
    let result = value(read_nodes_snapshot_for_call(&state, &arguments).call(&arguments));
    assert_eq!(result["nodes"][0]["children"], "...");
    assert_eq!(result["nodes"][1]["name"], "First match");
    assert_eq!(result["nodes"][2]["name"], "First match");
}

#[test]
fn request_batch_get_preserves_search_parent_order_refs_and_error_precedence() {
    let state = fixture();
    for arguments in [
        args(&[]),
        args(&[("nodeIds", "[\"two\",\"dup\",\"dup\"]"), ("readDepth", "0")]),
        args(&[
            ("patterns", "[{\"name\":\"match\"}]"),
            ("nodeIds", "two,dup"),
        ]),
        args(&[
            ("parentId", "two"),
            ("patterns", "[{\"name\":\"match\"}]"),
            ("searchDepth", "0"),
        ]),
        args(&[("nodeIds", "token"), ("resolveRefs", "true")]),
        args(&[("page_id", "other"), ("ids", "elsewhere")]),
        args(&[("page", "missing"), ("readDepth", "invalid")]),
        args(&[("readDepth", "invalid")]),
        args(&[("patterns", "[{\"name\":\"[\"}]")]),
        args(&[("nodeIds", "[1]")]),
    ] {
        assert_eq!(
            batch_get_snapshot_for_call(&state, &arguments).call(&arguments),
            batch_get_snapshot(&state).call(&arguments),
            "{arguments:?}"
        );
    }
    let arguments = args(&[
        ("parentId", "two"),
        ("patterns", "[{\"name\":\"match\"}]"),
        ("nodeIds", "dup"),
    ]);
    let result = value(batch_get_snapshot_for_call(&state, &arguments).call(&arguments));
    assert_eq!(result["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(result["nodes"][0]["name"], "Second match");
    let arguments = args(&[("nodeIds", "token"), ("resolveRefs", "true")]);
    assert_eq!(
        value(batch_get_snapshot_for_call(&state, &arguments).call(&arguments))["nodes"][0]["fill"]
            [0]["color"],
        "#3366ff"
    );
}

#[test]
fn request_reads_remain_detached_after_document_and_theme_mutations() {
    let mut state = fixture();
    let arguments = args(&[
        ("nodeIds", "token"),
        ("includeVariables", "true"),
        ("resolveRefs", "true"),
    ]);
    let read = read_nodes_snapshot_for_call(&state, &arguments);
    let batch = batch_get_snapshot_for_call(&state, &arguments);
    let expected_read = read_nodes_snapshot(&state).call(&arguments);
    let expected_batch = batch_get_snapshot(&state).call(&arguments);
    state.doc.pages.as_mut().unwrap()[0].children[2]
        .base_mut()
        .name = Some("Changed later".into());
    state.doc.variables = None;
    assert_eq!(read.call(&arguments), expected_read);
    assert_eq!(batch.call(&arguments), expected_batch);
}

#[test]
fn request_reads_keep_implicit_page_and_absent_metadata_shapes() {
    let state = EditorState::from_document(jian_ops_schema::load_str(r#"{
        "version":"1.0.0","pages":[],"children":[{"type":"rectangle","id":"plain","width":10,"height":10}]}
    "#).unwrap().value);
    let arguments = args(&[
        ("pageId", "0"),
        ("nodeIds", "plain"),
        ("includeVariables", "true"),
    ]);
    let result = value(read_nodes_snapshot_for_call(&state, &arguments).call(&arguments));
    assert_eq!(result["variables"], json!({}));
    assert_eq!(result["themes"], json!([]));
    assert_eq!(result["nodes"][0]["id"], "plain");
    assert_eq!(
        batch_get_snapshot_for_call(&state, &arguments).call(&arguments),
        batch_get_snapshot(&state).call(&arguments)
    );
}
