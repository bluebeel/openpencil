use super::*;
use op_editor_core::{EditorCommand, NodeId};

const PROFILE: McpAccessProfile = McpAccessProfile::UNRESTRICTED;

#[test]
fn protocol_control_messages_bypass_tool_routing_and_agent_contexts() {
    // No capacity: any accidental checkout/admission fails the regression.
    let store = AgentContexts::with_limits(0, 0, 0);
    let mut state = EditorState::new();
    let path = Path::new("/control-messages-must-not-open-this/document.op");
    for profile in [PROFILE, McpAccessProfile::LEAN] {
        for method in [
            "initialize",
            "tools/list",
            "notifications/initialized",
            "initialized",
            "ping",
        ] {
            let line = serde_json::json!({"jsonrpc":"2.0","id":37,"method":method,
                "params":{"agentId":"invalid agent id", "filePath":path.to_str().unwrap()}})
            .to_string();
            let admission = store.try_admit(&line, profile).unwrap();
            assert!(!admission.is_active());
            assert!(
                super::super::file_path::process_message_for_file_path_arg_with_contexts(
                    &store,
                    Some(path),
                    &line,
                    profile,
                )
                .unwrap()
                .is_none()
            );
            let response = with_agent_context(&store, path, &mut state, &line, profile, |state| {
                super::super::process_message_with_applier_profiled(
                    state,
                    &line,
                    profile,
                    |_, _, _| panic!("control message applied an editor command"),
                )
            })
            .unwrap();
            if matches!(method, "notifications/initialized" | "initialized") {
                assert!(response.is_none());
            } else {
                let wire: serde_json::Value = serde_json::from_str(&response.unwrap()).unwrap();
                assert_eq!(wire["id"], 37);
                assert!(wire.get("error").is_none(), "{wire}");
                if method == "tools/list" {
                    assert!(wire["result"]["tools"].is_array());
                }
            }
        }
    }
}

fn fixture() -> EditorState {
    EditorState::from_document(
        jian_ops_schema::load_str(
            r#"{
      "version":"1.0.0","pages":[
        {"id":"p1","name":"One","children":[
          {"type":"rectangle","id":"n1","name":"First","width":10,"height":10},
          {"type":"rectangle","id":"n2","name":"Second","width":10,"height":10}]},
        {"id":"p2","name":"Two","children":[
          {"type":"rectangle","id":"n3","name":"Live","width":10,"height":10}]}]
    }"#,
        )
        .unwrap()
        .value,
    )
}

fn request(agent: Option<&str>, tool: &str, arguments: serde_json::Value) -> String {
    let mut arguments = arguments;
    if let Some(agent) = agent {
        arguments["agentId"] = agent.into();
    }
    serde_json::json!({"jsonrpc":"2.0","id":"test","method":"tools/call",
        "params":{"name":tool,"arguments":arguments}})
    .to_string()
}

fn call(
    store: &AgentContexts,
    document: &str,
    state: &mut EditorState,
    agent: Option<&str>,
    tool: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let line = request(agent, tool, arguments);
    let _admission = store.try_admit(&line, PROFILE).unwrap();
    let response = with_agent_context(store, Path::new(document), state, &line, PROFILE, |state| {
        super::super::process_message_with_applier_profiled(
            state,
            &line,
            PROFILE,
            |_, state, cmd| state.apply(cmd.clone()),
        )
    })
    .unwrap()
    .unwrap();
    let wire: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_ne!(wire["result"]["isError"], true, "{wire}");
    serde_json::from_str(wire["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn agents_keep_independent_selection_clipboard_viewport_and_active_page() {
    let store = AgentContexts::default();
    let mut state = fixture();
    state.ui.active_page_index = 1;
    state.set_single_selection(NodeId::new("n3"));
    state.viewport.zoom = 1.75;
    state.tool = op_editor_core::tool::Tool::Pen;
    state.ui.property_input_draft = "human draft".into();
    state.editor_ui.image_crop_editing = Some(NodeId::new("n3"));
    state.clipboard = state.active_children().to_vec();
    call(
        &store,
        "doc",
        &mut state,
        Some("A"),
        "set_selection",
        serde_json::json!({"node_id":"n1"}),
    );
    call(
        &store,
        "doc",
        &mut state,
        Some("B"),
        "set_selection",
        serde_json::json!({"node_id":"n2"}),
    );
    call(
        &store,
        "doc",
        &mut state,
        Some("A"),
        "copy_selected",
        serde_json::json!({}),
    );
    call(
        &store,
        "doc",
        &mut state,
        Some("B"),
        "copy_selected",
        serde_json::json!({}),
    );
    call(
        &store,
        "doc",
        &mut state,
        Some("A"),
        "paste_clipboard",
        serde_json::json!({"offset_px":"0"}),
    );
    let pages = state.doc.pages.as_ref().unwrap();
    assert_eq!(pages[0].children.len(), 3);
    let pasted = pages[0]
        .children
        .iter()
        .find(|node| !["n1", "n2"].contains(&node.id_str()))
        .unwrap();
    assert_eq!(pasted.base().name.as_deref(), Some("First"));
    call(
        &store,
        "doc",
        &mut state,
        Some("A"),
        "set_viewport",
        serde_json::json!({"zoom_percent":"125"}),
    );
    call(
        &store,
        "doc",
        &mut state,
        Some("B"),
        "set_viewport",
        serde_json::json!({"zoom_percent":"200"}),
    );
    let viewport = call(
        &store,
        "doc",
        &mut state,
        Some("A"),
        "get_viewport",
        serde_json::json!({}),
    );
    assert_eq!(viewport["zoom_percent"], "125");
    call(
        &store,
        "doc",
        &mut state,
        Some("A"),
        "set_active_page",
        serde_json::json!({"index":"1"}),
    );
    let line = request(Some("B"), "get_viewport", serde_json::json!({}));
    with_agent_context(
        &store,
        Path::new("doc"),
        &mut state,
        &line,
        PROFILE,
        |state| {
            assert_eq!(state.ui.active_page_index, 0);
            Ok(None)
        },
    )
    .unwrap();
    assert_eq!(state.ui.active_page_index, 1);
    assert_eq!(state.selection.anchor.as_str(), "n3");
    call(
        &store,
        "doc",
        &mut state,
        Some("A"),
        "set_active_tool",
        serde_json::json!({"tool":"text"}),
    );
    let tool_line = request(Some("A"), "get_viewport", serde_json::json!({}));
    with_agent_context(
        &store,
        Path::new("doc"),
        &mut state,
        &tool_line,
        PROFILE,
        |state| {
            assert_eq!(state.tool, op_editor_core::tool::Tool::Text);
            Ok(None)
        },
    )
    .unwrap();
    assert_eq!(state.tool, op_editor_core::tool::Tool::Pen);
    assert_eq!(state.viewport.zoom, 1.75);
    assert_eq!(state.clipboard[0].id_str(), "n3");
    assert_eq!(state.ui.property_input_draft, "human draft");
    assert_eq!(state.editor_ui.image_crop_editing, Some(NodeId::new("n3")));
    assert!(!state.history.past.is_empty(), "document history is shared");
}

#[test]
fn clipboard_crosses_documents_only_for_the_same_agent() {
    let store = AgentContexts::default();
    let mut source = fixture();
    let mut destination = fixture();
    call(
        &store,
        "source",
        &mut source,
        Some("A"),
        "set_selection",
        serde_json::json!({"node_id":"n1"}),
    );
    call(
        &store,
        "source",
        &mut source,
        Some("A"),
        "copy_selected",
        serde_json::json!({}),
    );
    call(
        &store,
        "destination",
        &mut destination,
        Some("A"),
        "paste_clipboard",
        serde_json::json!({"offset_px":"0"}),
    );
    assert_eq!(destination.doc.pages.as_ref().unwrap()[0].children.len(), 3);
    let ledger = store.ledger.lock().unwrap();
    let agent = ledger.agents["A"].data.as_ref().unwrap();
    assert_eq!(agent.documents.len(), 2);
    assert_eq!(agent.clipboard[0].id_str(), "n1");
    assert_eq!(
        agent.documents[Path::new("source")]
            .selection
            .anchor
            .as_str(),
        "n1"
    );
    assert_ne!(
        agent.documents[Path::new("destination")]
            .selection
            .anchor
            .as_str(),
        "n1"
    );
}

#[test]
fn legacy_calls_errors_and_panics_restore_the_live_context_without_undoing_writes() {
    let store = AgentContexts::default();
    let mut state = fixture();
    call(
        &store,
        "doc",
        &mut state,
        None,
        "set_viewport",
        serde_json::json!({"zoom_percent":"150"}),
    );
    assert_eq!(state.viewport.zoom, 1.5);
    assert!(store.ledger.lock().unwrap().agents.is_empty());
    let line = request(Some("A"), "set_viewport", serde_json::json!({}));
    let response = with_agent_context(
        &store,
        Path::new("doc"),
        &mut state,
        &line,
        PROFILE,
        |state| {
            state.viewport.zoom = 2.5;
            state.apply(EditorCommand::SetSelection {
                node_id: NodeId::new("n1"),
            });
            panic!("synthetic tool panic");
        },
    )
    .unwrap()
    .unwrap();
    assert!(response.contains("tool panicked"));
    assert_eq!(state.viewport.zoom, 1.5);
    assert!(state.selection.is_empty());
    assert!(
        store.try_admit(&line, PROFILE).is_ok(),
        "panic releases busy ownership"
    );
    let result = with_agent_context(
        &store,
        Path::new("doc"),
        &mut state,
        &line,
        PROFILE,
        |state| {
            state.viewport.zoom = 3.0;
            state.doc.name = Some("shared canonical write".into());
            Err(McpServeError::Document("synthetic save error".into()))
        },
    );
    assert!(result.is_err());
    assert_eq!(state.viewport.zoom, 1.5);
    assert_eq!(state.doc.name.as_deref(), Some("shared canonical write"));
}

#[test]
fn admission_is_bounded_and_rejects_same_agent_before_document_locking() {
    let store = AgentContexts::with_limits(1, 1, 200_000);
    let line = request(Some("A"), "get_viewport", serde_json::json!({}));
    let admission = store.try_admit(&line, PROFILE).unwrap();
    assert!(matches!(
        store.try_admit(&line, PROFILE),
        Err(AgentContextError::Busy)
    ));
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                assert!(matches!(
                    store.try_admit(&line, PROFILE),
                    Err(AgentContextError::Busy)
                ));
            })
            .join()
            .unwrap()
    });
    drop(admission);
    let mut state = fixture();
    with_agent_context(
        &store,
        Path::new("first"),
        &mut state,
        &line,
        PROFILE,
        |_| Ok(None),
    )
    .unwrap();
    let response = with_agent_context(
        &store,
        Path::new("second"),
        &mut state,
        &line,
        PROFILE,
        |_| panic!("capacity must refuse first"),
    )
    .unwrap()
    .unwrap();
    assert!(response.contains("documents per agent"));
    let second = request(Some("B"), "get_viewport", serde_json::json!({}));
    assert!(matches!(
        store.try_admit(&second, PROFILE),
        Err(AgentContextError::Capacity {
            resource: "agents",
            ..
        })
    ));
    let invalid = request(Some("../invalid"), "get_viewport", serde_json::json!({}));
    assert!(matches!(
        store.try_admit(&invalid, PROFILE),
        Err(AgentContextError::InvalidAgent)
    ));
    let tiny = AgentContexts::with_limits(1, 1, 128);
    assert!(matches!(
        tiny.try_admit(&line, PROFILE),
        Err(AgentContextError::Capacity { .. })
    ));
}

#[test]
fn oversized_cut_is_rejected_before_clipboard_or_document_mutation() {
    let store = AgentContexts::default();
    let mut state = fixture();
    let mut node = state.doc.pages.as_ref().unwrap()[0].children[0].clone();
    node.base_mut().name = Some("x".repeat(MAX_CLIPBOARD));
    state.doc.pages.as_mut().unwrap()[0].children[0] = node;
    call(
        &store,
        "doc",
        &mut state,
        Some("A"),
        "set_selection",
        serde_json::json!({"node_id":"n1"}),
    );
    let line = request(Some("A"), "cut_selected", serde_json::json!({}));
    let response = with_agent_context(&store, Path::new("doc"), &mut state, &line, PROFILE, |_| {
        panic!("oversized cut must not dispatch")
    })
    .unwrap()
    .unwrap();
    assert!(response.contains("agent-context-capacity"));
    assert_eq!(state.doc.pages.as_ref().unwrap()[0].children.len(), 2);
}

#[test]
fn agent_context_discovery_advertises_optional_context_without_changing_required_args() {
    let state = fixture();
    let response = super::super::tools_list::tools_list_response("1", &state, false, PROFILE);
    let json: serde_json::Value = serde_json::from_str(&response).unwrap();
    for tool in json["result"]["tools"].as_array().unwrap() {
        assert_eq!(
            tool["inputSchema"]["properties"]["agentId"]["maxLength"],
            96
        );
        assert!(!tool["inputSchema"]["required"]
            .as_array()
            .is_some_and(|required| required.iter().any(|key| key == "agentId")));
    }
}
