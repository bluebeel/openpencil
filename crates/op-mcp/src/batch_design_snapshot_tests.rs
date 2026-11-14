use super::*;
use crate::test_fixtures::{frame, state_with, text};

fn fixture() -> EditorState {
    let mut state = state_with(vec![frame(
        "parent",
        "Parent",
        0.0,
        0.0,
        100.0,
        100.0,
        vec![text("label", "Label", 0.0, 0.0, 20.0, 20.0, "Text")],
    )]);
    state.chat.title = "unrelated transcript".repeat(1024);
    state
}

fn args(key: &str, value: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(key.into(), value.into())])
}

#[test]
fn batch_snapshot_stateless_payloads_retain_no_document_and_preserve_results() {
    let state = fixture();
    for args in [
        args(
            "nodes_json",
            r#"[{"kind":"rect","name":"A","width":10,"height":20}]"#,
        ),
        args("operations", r#"U("label", {"content":"Updated"})"#),
        args("nodes_json", "malformed"),
        BTreeMap::new(),
    ] {
        let scoped = batch_design_snapshot_for_call(&state, &args);
        assert!(scoped.state.is_none());
        assert!(scoped.pages.is_empty());
        assert!(scoped.existing_ids.is_empty());
        assert_eq!(scoped.id_seed, None);
        assert_eq!(
            scoped.call(&args),
            batch_design_snapshot(&state).call(&args)
        );
    }
    assert_eq!(state.active_children().len(), 1);
}

#[test]
fn batch_snapshot_inserts_retain_only_ids_and_page_counts() {
    let mut state = fixture();
    let args = args(
        "operations",
        r#"card=I("parent", {"type":"frame","name":"New","width":10,"height":20})"#,
    );
    let scoped = batch_design_snapshot_for_call(&state, &args);
    assert!(scoped.state.is_none());
    assert_eq!(scoped.pages[0].count, 2);
    assert_eq!(scoped.existing_ids.len(), state.collect_node_ids().len());
    let outcome = scoped.call(&args);
    assert_eq!(outcome, batch_design_snapshot(&state).call(&args));
    let ToolOutcome::OkJsonWithCommand(json, cmd) = outcome else {
        panic!("insert outcome")
    };
    assert_eq!(
        serde_json::from_str::<Value>(&json).unwrap()["nodeCount"],
        3
    );
    assert!(state.apply(cmd));
    assert_eq!(count_forest(state.active_children()), 3);
}

#[test]
fn batch_snapshot_mixed_program_retains_transaction_and_rejects_atomically() {
    let state = fixture();
    let args = args(
        "operations",
        r#"U("label", {"content":"Updated"}); D("missing")"#,
    );
    let scoped = batch_design_snapshot_for_call(&state, &args);
    assert!(scoped.state.is_some());
    assert!(scoped.pages.is_empty());
    assert!(scoped.existing_ids.is_empty());
    let actual = scoped.call(&args);
    assert_eq!(actual, batch_design_snapshot(&state).call(&args));
    let ToolOutcome::OkJson(json) = actual else {
        panic!("rollback emits no command")
    };
    let json: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json["applied"], false);
    assert_eq!(json["nodeCount"], 2);
    assert_eq!(json["results"], serde_json::json!([]));
    assert_eq!(state.doc, fixture().doc);
}

#[test]
fn batch_snapshot_ambiguous_and_script_inputs_keep_validation_contract() {
    let state = fixture();
    for args in [
        BTreeMap::from([
            ("operations".into(), "D(\"label\")".into()),
            ("nodes_json".into(), "[{\"kind\":\"rect\"}]".into()),
        ]),
        args(
            "script",
            "const x = I(null, {type:'frame', name:'A', width:20, height:20});",
        ),
    ] {
        let scoped = batch_design_snapshot_for_call(&state, &args);
        assert_eq!(
            scoped.call(&args),
            batch_design_snapshot(&state).call(&args)
        );
    }
}

#[cfg(feature = "script")]
#[test]
fn batch_snapshot_script_inserts_expand_once_without_editor_snapshot() {
    let state = fixture();
    let args = args(
        "script",
        "const x = I(null, {type:'text',content:String(Math.random()),width:20,height:20});",
    );
    let tool = batch_design_snapshot_for_call(&state, &args);
    assert!(tool.state.is_none());
    assert_eq!(tool.pages[0].count, 2);
    let prepared = tool.prepared.as_ref().unwrap().expanded.as_ref().unwrap();
    assert!(!prepared.contains_key("script"));
    let first = tool.call(&args);
    assert!(matches!(first, ToolOutcome::OkJsonWithCommand(_, _)));
    assert_eq!(
        first,
        tool.call(&args),
        "random script result is cached, never reevaluated"
    );
    assert_eq!(first, tool.call_uncached(prepared, None));
    let changed = self::args("script", "throw new Error('must not execute');");
    let ToolOutcome::Err(code, message) = tool.call(&changed) else {
        panic!("request mismatch")
    };
    assert_eq!(code, ToolErrorCode::InvalidArgument);
    assert!(message.contains("different arguments"));
}

#[cfg(feature = "script")]
#[test]
fn batch_snapshot_mixed_script_keeps_atomic_rollback_and_cached_errors() {
    let state = fixture();
    let args = args(
        "script",
        "U('label',{content:'Updated'}); U('missing',{content:'Reject'});",
    );
    let tool = batch_design_snapshot_for_call(&state, &args);
    assert!(tool.state.is_some());
    let actual = tool.call(&args);
    assert_eq!(actual, batch_design_snapshot(&state).call(&args));
    let ToolOutcome::OkJson(json) = actual else {
        panic!("rollback has no command")
    };
    assert_eq!(
        serde_json::from_str::<Value>(&json).unwrap()["applied"],
        false
    );
    let args = self::args("script", "throw new Error('same cached error');");
    let tool = batch_design_snapshot_for_call(&state, &args);
    assert!(tool.state.is_none());
    assert!(tool.existing_ids.is_empty());
    assert_eq!(tool.call(&args), batch_design_snapshot(&state).call(&args));
}
