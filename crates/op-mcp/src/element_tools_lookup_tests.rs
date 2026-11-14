use super::*;

#[test]
fn targeted_component_lookup_matches_full_registry_including_name_collisions() {
    let mut state = EditorState::new();
    let args = BTreeMap::from([("x".into(), "12.5".into()), ("y".into(), "34.5".into())]);
    let mut expected = BTreeMap::new();
    for tool in insert_kit_component_tools(&state) {
        expected.insert(tool.name().to_owned(), tool.call(&args));
    }
    assert!(!expected.is_empty());
    for (name, outcome) in expected {
        assert_eq!(
            insert_kit_component_tool_for_name(&state, &name)
                .unwrap()
                .call(&args),
            outcome
        );
    }
    let mut first = state.ui_kits[0].clone();
    first.id = "first".into();
    first.components.truncate(1);
    first.components[0].id = "custom-button".into();
    let mut last = first.clone();
    last.id = "last".into();
    last.components[0].id = "custom button".into();
    state.ui_kits = vec![first, last];
    let expected = insert_kit_component_tools(&state)
        .pop()
        .unwrap()
        .call(&args);
    assert_eq!(
        insert_kit_component_tool_for_name(&state, "insert_custom_button")
            .unwrap()
            .call(&args),
        expected
    );
    for unknown in [
        "add_custom_button",
        "insert_missing",
        "insert_",
        "insert_custom按钮",
    ] {
        assert!(insert_kit_component_tool_for_name(&state, unknown).is_none());
    }
}

#[test]
fn kit_coordinates_reject_nonfinite_input_before_emitting_a_command() {
    let state = EditorState::new();
    let tools = insert_kit_component_tools(&state);
    let tool = &tools[0];
    for key in ["x", "y"] {
        for value in ["NaN", "inf", "-inf", "1e999"] {
            let args = BTreeMap::from([(key.into(), value.into())]);
            assert!(
                matches!(
                    tool.call(&args),
                    ToolOutcome::Err(ToolErrorCode::InvalidArgument, _)
                ),
                "{key}={value}"
            );
        }
    }
    assert!(matches!(
        tool.call(&BTreeMap::new()),
        ToolOutcome::OkWithCommand(_, _)
    ));
}
