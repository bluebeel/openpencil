use super::*;

#[test]
fn resident_busy_agent_is_refused_before_waiting_for_the_primary_writer() {
    let canvas = fresh_state();
    let authority = ConnAuthority::new(&canvas);
    let body = serde_json::json!({"jsonrpc":"2.0","id":74,"method":"tools/call","params":{"name":"get_selection","arguments":{"agentId":"already-active"}}}).to_string();
    let _active = authority
        .agent_contexts
        .try_admit(
            &body,
            crate::mcp_serve::tool_profile::McpAccessProfile::UNRESTRICTED,
        )
        .unwrap();
    let state = Mutex::new(canvas);
    let held_writer = state.lock().unwrap();
    let (sent, receive) = std::sync::mpsc::channel();
    let result = std::thread::scope(|scope| {
        let state = &state;
        let authority = &authority;
        scope.spawn(move || {
            let frame = format!(
                "POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let mut stream = mock_stream(&frame);
            serve_one_with_authority(
                &mut stream,
                state,
                &SseHub::default(),
                ServeMode::Local,
                authority,
            )
            .unwrap();
            sent.send(String::from_utf8(stream.output).unwrap())
                .unwrap();
        });
        let result = receive.recv_timeout(std::time::Duration::from_secs(2));
        // Always release before joining, including the failure path, so an
        // accidental document-lock wait fails rather than hanging the suite.
        drop(held_writer);
        result
    });
    let response = result.expect("busy rejection must not wait for document writer");
    assert!(
        response.contains("busy") && response.contains("200 OK"),
        "{response}"
    );
}
