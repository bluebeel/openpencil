use super::*;
use crate::test_fixtures::{frame, state_with, text};
use crate::{batch_design_snapshot, batch_design_snapshot_for_call};

fn fixture() -> EditorState {
    state_with(vec![frame(
        "parent",
        "Parent",
        0.0,
        0.0,
        100.0,
        100.0,
        vec![text("label", "Label", 0.0, 0.0, 20.0, 20.0, "Text")],
    )])
}

fn args(key: &str, value: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(key.into(), value.into())])
}

#[test]
fn prepared_call_matches_reusable_outcomes_without_mutating_the_source() {
    let mut state = fixture();
    state.chat.title = "Unrelated conversation".repeat(1024);
    let before = serde_json::to_vec(&state.doc).unwrap();
    let selection = state.selection.clone();
    for arguments in [
        args(
            "nodes_json",
            r#"[{"kind":"rect","name":"A","width":10,"height":20}]"#,
        ),
        args(
            "operations",
            r#"card=I("parent", {"type":"frame","name":"New","width":10,"height":20})"#,
        ),
        args("operations", r#"U("label", {"content":"Updated"})"#),
        args(
            "operations",
            r#"card=I("parent", {"type":"frame","width":10,"height":20});
U(card,{"name":"Changed"});
U("label",{"content":"Updated"})"#,
        ),
        args(
            "operations",
            r#"U("label", {"content":"Updated"});
D("missing")"#,
        ),
        args("operations", r#"G("label", "A landscape")"#),
        args("nodes_json", "malformed"),
        BTreeMap::new(),
        BTreeMap::from([
            ("operations".into(), "D(\"label\")".into()),
            ("nodes_json".into(), "[{\"kind\":\"rect\"}]".into()),
        ]),
    ] {
        let expected = batch_design_snapshot(&state).call(&arguments);
        let prepared = prepare_batch_design_call(&state, &arguments);
        assert_eq!(prepared.call(&arguments), expected, "{arguments:?}");
        assert_eq!(serde_json::to_vec(&state.doc).unwrap(), before);
        assert_eq!(state.selection, selection);
        assert!(state.history.past.is_empty());
    }
}

#[test]
fn prepared_request_rejects_other_arguments_without_consuming_the_command() {
    let state = fixture();
    let original = args(
        "operations",
        r#"U("label", {"content":"Updated"});
U("label", {"name":"Changed"})"#,
    );
    let prepared = prepare_batch_design_call(&state, &original);
    let different = args("operations", "D(\"label\")");
    assert!(matches!(
        prepared.call(&different),
        ToolOutcome::Err(ToolErrorCode::InvalidArgument, _)
    ));
    assert_eq!(
        prepared.call(&original),
        batch_design_snapshot(&state).call(&original)
    );
    assert!(matches!(
        prepared.call(&original),
        ToolOutcome::Err(ToolErrorCode::ToolFailed, _)
    ));
    // Reusable public snapshot APIs remain reusable with identical arguments.
    let reusable = batch_design_snapshot_for_call(&state, &original);
    assert_eq!(reusable.call(&original), reusable.call(&original));
}

#[test]
fn prepared_mixed_program_matches_page_and_best_effort_contracts() {
    let state = fixture();
    for extra in [
        ("pageId", "missing"),
        ("_line_policy", "best_effort"),
        ("postProcess", "true"),
    ] {
        let mut arguments = args(
            "operations",
            r#"U("label", {"content":"Updated"});
D("missing")"#,
        );
        arguments.insert(extra.0.into(), extra.1.into());
        assert_eq!(
            prepare_batch_design_call(&state, &arguments).call(&arguments),
            batch_design_snapshot(&state).call(&arguments)
        );
    }
}

#[cfg(feature = "script")]
#[test]
fn prepared_script_preserves_the_expanded_program_contract() {
    let state = fixture();
    let arguments = args(
        "script",
        r#"const card=I("parent",{type:"frame",width:10,height:20});
U(card,{name:"Changed"});"#,
    );
    assert_eq!(
        prepare_batch_design_call(&state, &arguments).call(&arguments),
        batch_design_snapshot_for_call(&state, &arguments).call(&arguments)
    );
    assert!(matches!(
        prepare_batch_design_call(&state, &arguments).call(&arguments),
        ToolOutcome::OkJsonWithCommand(_, _)
    ));
}

#[test]
fn repeated_copy_delete_keeps_live_source_and_child_references() {
    let mut state = fixture();
    let before = serde_json::to_vec(&state.doc).unwrap();
    let arguments = args(
        "operations",
        r#"
        first=C("parent", null, {"name":"First copy"});
        U(first,{"x":12});
D(first);
        second=C("parent", null, {"name":"Second copy"});
        U("parent",{"name":"Original remains live"});
        U("label",{"content":"Original child remains live"});
D(second)
    "#,
    );
    let outcome = prepare_batch_design_call(&state, &arguments).call(&arguments);
    assert_eq!(outcome, batch_design_snapshot(&state).call(&arguments));
    let ToolOutcome::OkJsonWithCommand(result, command) = outcome else {
        panic!("copy program must produce a command: {outcome:?}")
    };
    let result: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(result.get("errors").is_none(), "{result}");
    assert!(state.apply_undoable(command));
    assert_eq!(state.doc.children.len(), 1);
    let root = serde_json::to_value(&state.doc.children[0]).unwrap();
    assert_eq!(root["name"], "Original remains live");
    assert_eq!(
        root["children"][0]["content"],
        "Original child remains live"
    );
    let after = serde_json::to_vec(&state.doc).unwrap();
    assert!(state.apply(crate::EditorCommand::Undo));
    assert_eq!(serde_json::to_vec(&state.doc).unwrap(), before);
    assert!(state.apply(crate::EditorCommand::Redo));
    assert_eq!(serde_json::to_vec(&state.doc).unwrap(), after);
}
