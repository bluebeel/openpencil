//! The record tools deliberately have different lookup and duplicate policies.

use std::collections::BTreeMap;

use op_editor_core::{EditorState, NodeId, PenNodeExt};

use crate::{
    find_node_by_name_snapshot, find_node_by_name_snapshot_for_call, get_node_children_snapshot,
    get_node_children_snapshot_for_call, get_node_parent_snapshot,
    get_node_parent_snapshot_for_call, get_node_snapshot, get_node_snapshot_for_call, McpTool,
    ToolOutcome,
};

fn fixture() -> EditorState {
    let mut state = EditorState::from_document(
        jian_ops_schema::load_str(
            r#"{
      "version":"1.0.0","pages":[{"id":"first","name":"First","children":[
        {"type":"frame","id":"dup","name":"Outer","width":100,"height":100,"children":[
          {"type":"rectangle","id":"dup","name":"Nested","width":10,"height":10},
          {"type":"frame","id":"container","name":"Inner","width":20,"height":20,"children":[
            {"type":"rectangle","id":"inner-child","name":"Match","width":5,"height":5}]}]},
        {"type":"frame","id":"container","name":"Later","width":30,"height":30,"children":[
          {"type":"rectangle","id":"later-child","name":"Match","width":6,"height":7}]},
        {"type":"rectangle","id":"container","name":"Tail leaf","width":1,"height":1},
        {"type":"rectangle","id":"unnamed","width":1,"height":1},
        {"type":"rectangle","id":" spaced ","name":" spaced ","width":2,"height":2}]},
        {"id":"other","name":"Other","children":[
          {"type":"rectangle","id":"dup","name":"Other page","width":40,"height":50}]}]
    }"#,
        )
        .unwrap()
        .value,
    );
    state
        .ui
        .variables
        .fill_refs
        .insert(NodeId::new("dup"), "brand".into());
    state
        .ui
        .variables
        .stroke_refs
        .insert(NodeId::new("dup"), "outline".into());
    state
}

fn args(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn fields(outcome: ToolOutcome) -> BTreeMap<String, String> {
    match outcome {
        ToolOutcome::Ok(fields) => fields,
        other => panic!("{other:?}"),
    }
}

#[test]
fn requested_get_node_is_global_last_preorder_match_with_exact_cached_refs() {
    let mut state = fixture();
    for arguments in [
        args(&[]),
        args(&[("nodeId", "dup")]),
        args(&[("node_id", "dup")]),
        args(&[("node_id", "container")]),
        args(&[("node_id", "missing")]),
        args(&[("node_id", " spaced ")]),
        args(&[("node_id", "spaced")]),
        args(&[("node_id", "dup"), ("pageId", "missing")]),
    ] {
        assert_eq!(
            get_node_snapshot_for_call(&state, &arguments).call(&arguments),
            get_node_snapshot(&state).call(&arguments),
            "{arguments:?}"
        );
    }
    let arguments = args(&[("node_id", "dup")]);
    let record = fields(get_node_snapshot_for_call(&state, &arguments).call(&arguments));
    assert_eq!(record["name"], "Other page");
    assert_eq!(record["parent_id"], "");
    assert_eq!(record["width"], "40");
    assert_eq!(record["fill_ref"], "brand");
    assert_eq!(record["stroke_ref"], "outline");
    state.doc.pages.as_mut().unwrap()[1].children[0]
        .base_mut()
        .id = String::new();
    let empty = args(&[("node_id", "")]);
    assert_eq!(
        get_node_snapshot_for_call(&state, &empty).call(&empty),
        get_node_snapshot(&state).call(&empty)
    );
    assert_eq!(
        fields(get_node_snapshot_for_call(&state, &empty).call(&empty))["name"],
        "Other page"
    );
}

#[test]
fn requested_children_keep_nonempty_postorder_winner_and_known_leaf_semantics() {
    let state = fixture();
    for arguments in [
        args(&[]),
        args(&[("nodeId", "dup")]),
        args(&[("node_id", "dup")]),
        args(&[("node_id", "container")]),
        args(&[("node_id", "unnamed")]),
        args(&[("node_id", "missing")]),
        args(&[("node_id", " spaced ")]),
    ] {
        assert_eq!(
            get_node_children_snapshot_for_call(&state, &arguments).call(&arguments),
            get_node_children_snapshot(&state).call(&arguments),
            "{arguments:?}"
        );
    }
    let arguments = args(&[("node_id", "dup")]);
    let record = fields(get_node_children_snapshot_for_call(&state, &arguments).call(&arguments));
    assert_eq!(
        record["ids"], "dup,container",
        "outer container wins over its same-ID descendant; later leaf does not erase it"
    );
    let arguments = args(&[("node_id", "container")]);
    assert_eq!(
        fields(get_node_children_snapshot_for_call(&state, &arguments).call(&arguments))["ids"],
        "later-child"
    );
    let arguments = args(&[("node_id", "unnamed")]);
    assert_eq!(
        fields(get_node_children_snapshot_for_call(&state, &arguments).call(&arguments))["count"],
        "0"
    );
}

#[test]
fn requested_parent_and_name_use_active_page_first_exact_match() {
    let mut state = fixture();
    for active_page in [0, 1, 99] {
        state.ui.active_page_index = active_page;
        for arguments in [
            args(&[]),
            args(&[("nodeId", "dup")]),
            args(&[("node_id", "dup")]),
            args(&[("node_id", "container")]),
            args(&[("node_id", "inner-child")]),
            args(&[("node_id", "missing")]),
            args(&[("node_id", "dup"), ("pageId", "missing")]),
        ] {
            assert_eq!(
                get_node_parent_snapshot_for_call(&state, &arguments).call(&arguments),
                get_node_parent_snapshot(&state).call(&arguments),
                "{active_page}: {arguments:?}"
            );
        }
        for arguments in [
            args(&[]),
            args(&[("name", "Match")]),
            args(&[("name", "match")]),
            args(&[("name", "Other page")]),
            args(&[("name", "")]),
            args(&[("name", " spaced ")]),
        ] {
            assert_eq!(
                find_node_by_name_snapshot_for_call(&state, &arguments).call(&arguments),
                find_node_by_name_snapshot(&state).call(&arguments),
                "{active_page}: {arguments:?}"
            );
        }
    }
    state.ui.active_page_index = 0;
    let arguments = args(&[("node_id", "dup")]);
    let parent = fields(get_node_parent_snapshot_for_call(&state, &arguments).call(&arguments));
    assert_eq!(parent["parent_id"], "");
    assert_eq!(
        parent["depth"], "0",
        "first parent wins, unlike global get_node"
    );
    let arguments = args(&[("name", "Match")]);
    assert_eq!(
        fields(find_node_by_name_snapshot_for_call(&state, &arguments).call(&arguments))["id"],
        "inner-child"
    );
    let arguments = args(&[("name", "")]);
    assert_eq!(
        fields(find_node_by_name_snapshot_for_call(&state, &arguments).call(&arguments))["id"],
        "unnamed"
    );
}

#[test]
fn requested_records_preserve_some_empty_pages_vs_legacy_root_fallback() {
    let mut state = fixture();
    let roots = state.doc.pages.as_ref().unwrap()[0].children.clone();
    state.doc.children = roots;
    state.doc.pages = Some(Vec::new());
    let arguments = args(&[("node_id", "dup")]);
    assert_eq!(
        get_node_snapshot_for_call(&state, &arguments).call(&arguments),
        get_node_snapshot(&state).call(&arguments)
    );
    assert!(matches!(
        get_node_snapshot_for_call(&state, &arguments).call(&arguments),
        ToolOutcome::Err(..)
    ));
    assert_eq!(
        get_node_children_snapshot_for_call(&state, &arguments).call(&arguments),
        get_node_children_snapshot(&state).call(&arguments)
    );
    state.doc.pages = None;
    assert_eq!(
        get_node_snapshot_for_call(&state, &arguments).call(&arguments),
        get_node_snapshot(&state).call(&arguments)
    );
    assert_eq!(
        get_node_children_snapshot_for_call(&state, &arguments).call(&arguments),
        get_node_children_snapshot(&state).call(&arguments)
    );
    let tool = get_node_snapshot_for_call(&state, &arguments);
    let expected = get_node_snapshot(&state).call(&arguments);
    state.doc.children.clear();
    assert_eq!(
        tool.call(&arguments),
        expected,
        "one-node snapshot owns its detached record"
    );
}

#[test]
fn prepared_selection_preserves_duplicate_order_missing_ids_active_page_and_depth_aliases() {
    let mut state = fixture();
    state.selection.set = vec![
        NodeId::new("dup"),
        NodeId::new("unnamed"),
        NodeId::new("dup"),
        NodeId::new("missing"),
        NodeId::NONE,
    ];
    state.selection.anchor = NodeId::new("dup");
    for arguments in [
        args(&[]),
        args(&[("depth", "0")]),
        args(&[("readDepth", "1")]),
        args(&[("readDepth", "-1")]),
        args(&[("readDepth", "0"), ("depth", "invalid")]),
        args(&[("readDepth", "invalid"), ("depth", "0")]),
    ] {
        assert_eq!(
            crate::selection_snapshot_for_call(&state, &arguments).call(&arguments),
            crate::selection_snapshot(&state).call(&arguments),
            "{arguments:?}"
        );
    }
    let arguments = args(&[("depth", "0")]);
    let ToolOutcome::OkJson(json) =
        crate::selection_snapshot_for_call(&state, &arguments).call(&arguments)
    else {
        panic!("selection JSON")
    };
    let result: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        result["selectedIds"],
        serde_json::json!(["dup", "unnamed", "dup", "missing"])
    );
    assert_eq!(result["activePageId"], "first");
    assert_eq!(result["nodes"][0]["children"], "...");
    assert_eq!(result["nodes"][0], result["nodes"][2]);
    assert_eq!(result["nodes"].as_array().unwrap().len(), 3);
    assert!(result.get("selected_id").is_none());
    state.ui.active_page_index = 1;
    assert_eq!(
        crate::selection_snapshot_for_call(&state, &arguments).call(&arguments),
        crate::selection_snapshot(&state).call(&arguments)
    );
}

#[test]
fn prepared_selection_stays_detached_and_keeps_legacy_literal_construction_available() {
    let mut state = fixture();
    state.selection.set = vec![NodeId::new("dup")];
    state.selection.anchor = NodeId::new("dup");
    let arguments = args(&[("readDepth", "1")]);
    let tool = crate::selection_snapshot_for_call(&state, &arguments);
    let expected = crate::selection_snapshot(&state).call(&arguments);
    state.doc.pages = None;
    state.doc.children.clear();
    state.selection = Default::default();
    assert_eq!(tool.call(&arguments), expected);
    let literal = crate::GetSelection {
        selected_id: String::new(),
        kind: "none".into(),
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        selected_ids_json: "[]".into(),
        active_page_id: "0".into(),
        selected_nodes: Vec::new(),
    };
    assert_eq!(
        literal.call(&arguments),
        crate::selection_snapshot_for_call(&state, &arguments).call(&arguments)
    );
}
