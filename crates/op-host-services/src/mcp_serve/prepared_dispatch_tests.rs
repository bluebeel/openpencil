//! Every advertised kit tool must be executable through the real dispatcher.
use super::*;
use op_mcp::McpTool;

fn request(tool: &str, arguments: serde_json::Value) -> String {
    serde_json::json!({"jsonrpc":"2.0","id":19,"method":"tools/call",
        "params":{"name":tool,"arguments":arguments}})
    .to_string()
}

#[test]
fn every_advertised_kit_tool_inserts_through_mcp_and_undo_restores_exact_bytes() {
    let tools = op_mcp::element_tools::insert_kit_component_tools(&EditorState::new());
    assert!(!tools.is_empty());
    for tool in tools {
        let mut state = EditorState::new();
        let before = serde_json::to_vec(&state.doc).unwrap();
        let mut applied = 0;
        let response = process_message_with_applier_profiled(
            &mut state,
            &request(tool.name(), serde_json::json!({"x":"12.5","y":"34.5"})),
            tool_profile::McpAccessProfile::UNRESTRICTED,
            |_, state, cmd| {
                applied += 1;
                apply_command_with_undo(state, cmd.clone(), false)
            },
        )
        .unwrap()
        .unwrap();
        assert!(
            !response.contains(r#""isError":true"#),
            "{}: {response}",
            tool.name()
        );
        assert_eq!(applied, 1, "{}", tool.name());
        assert!(!state.doc.children.is_empty(), "{}", tool.name());
        assert_eq!(state.history.past.len(), 1, "{}", tool.name());
        let inserted = serde_json::to_vec(&state.doc).unwrap();
        assert!(state.apply(EditorCommand::Undo));
        assert_eq!(
            serde_json::to_vec(&state.doc).unwrap(),
            before,
            "{}",
            tool.name()
        );
        assert!(state.apply(EditorCommand::Redo));
        assert_eq!(
            serde_json::to_vec(&state.doc).unwrap(),
            inserted,
            "{}",
            tool.name()
        );
    }
}

#[test]
fn mixed_program_dispatch_preserves_atomicity_and_wire_result_in_both_catalogs() {
    for profile in [
        tool_profile::McpAccessProfile::UNRESTRICTED,
        tool_profile::McpAccessProfile::LEAN,
    ] {
        let mut state = EditorState::new();
        let valid = r#"card=I(null,{"type":"frame","name":"Card","width":20,"height":10});
U(card,{"name":"Changed"})"#;
        let mut applied = 0;
        let response = process_message_with_applier_profiled(
            &mut state,
            &request("batch_design", serde_json::json!({"operations":valid})),
            profile,
            |_, state, cmd| {
                applied += 1;
                apply_command_with_undo(state, cmd.clone(), false)
            },
        )
        .unwrap()
        .unwrap();
        assert!(!response.contains(r#""isError":true"#), "{response}");
        assert_eq!(applied, 1);
        let inserted = serde_json::to_vec(&state.doc).unwrap();
        let history = state.history.past.clone();
        let invalid = r#"card=I(null,{"type":"frame","name":"Reject","width":20,"height":10});
U("missing",{"x":1})"#;
        let response = process_message_with_applier_profiled(
            &mut state,
            &request("batch_design", serde_json::json!({"operations":invalid})),
            profile,
            |_, _, _| panic!("failed program must not reach the applier"),
        )
        .unwrap()
        .unwrap();
        let result: serde_json::Value = serde_json::from_str(&tool_text(&response)).unwrap();
        assert_eq!(result["applied"], false);
        assert_eq!(serde_json::to_vec(&state.doc).unwrap(), inserted);
        assert_eq!(state.history.past, history);
    }
}
