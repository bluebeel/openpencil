use super::*;

fn fixture() -> EditorState {
    EditorState::from_document(
        jian_ops_schema::load_str(
            r##"{
      "version":"1.0.0","variables":{"brand":{"type":"color","value":"#3366ff"}},
      "pages":[
       {"id":"library","name":"Library","children":[
        {"id":"master","type":"frame","reusable":true,"width":80,"height":40,
         "fill":[{"type":"solid","color":"$brand"}],"children":[
          {"id":"master-inner","type":"frame","width":40,"height":30,"children":[
           {"id":"master-leaf","type":"rectangle","x":3,"y":2,"width":6,"height":7}]}]}]},
       {"id":"p","name":"Measured","children":[
        {"id":"unrelated","type":"rectangle","width":10,"height":10},
        {"id":"root","type":"frame","x":101.5,"y":51.25,"width":500,"height":300,
         "rotation":13,"opacity":0.7,"children":[
          {"id":"measure","type":"frame","x":32.25,"y":24.5,"width":400,"height":"fit_content",
           "layout":"vertical","gap":8,"padding":[10,20],"children":[
            {"id":"title","type":"text","name":"Measured AV ffi","content":"Measured AV ffi",
             "fontFamily":"Inter","fontSize":18,"width":"fit_content","height":"fit_content"},
            {"id":"row","type":"frame","layout":"horizontal","width":"fill_container","height":60,
             "gap":6,"padding":4,"children":[
              {"id":"flex-a","type":"rectangle","width":"fill_container","height":12},
              {"id":"instance","type":"ref","ref":"master","width":80,"height":40}]}]}]}]},
       {"id":"other","name":"Other","children":[
        {"id":"other-root","type":"frame","x":900,"y":100,"width":60,"height":50,"children":[
          {"id":"other-leaf","type":"rectangle","x":12,"y":14,"width":20,"height":30}]}]}]
    }"##,
        )
        .unwrap()
        .value,
    )
}

fn arguments(value: Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string()),
            )
        })
        .collect()
}

#[test]
fn requested_layout_matches_full_measured_flex_components_pages_and_error_precedence() {
    let values = [
        json!({}),
        json!({"parentId":"measure","maxDepth":0}),
        json!({"parent_id":"row","depth":1}),
        json!({"parent":"instance__master-inner","depth":0}),
        json!({"page":"other","parent":"other-root","depth":0}),
        json!({"pageId":"library","parentId":"master","maxDepth":3}),
        json!({"parentId":"absent"}),
        json!({"maxDepth":"bad"}),
        json!({"pageId":"absent","maxDepth":"bad"}),
        json!({"parentId":""}),
    ];
    for preserve in [false, true] {
        for workspace in [false, true] {
            let mut state = fixture();
            state.ui.active_page_index = 1;
            state.editor_ui.preserve_authored_geometry = preserve;
            state.editor_ui.workspace.visible = workspace;
            let legacy = snapshot_layout_snapshot(&state);
            for value in &values {
                let args = arguments(value.clone());
                let focused = snapshot_layout_snapshot_for_call(&state, &args);
                assert!(
                    focused.state.is_none(),
                    "request must not retain EditorState"
                );
                assert_eq!(
                    focused.call(&args),
                    legacy.call(&args),
                    "{value}, preserve={preserve}, workspace={workspace}"
                );
            }
        }
    }
}

#[test]
fn requested_empty_space_matches_computed_bounds_and_validation_order() {
    let mut state = fixture();
    state.ui.active_page_index = 1;
    let values = [
        json!({"width":50,"height":30,"direction":"bottom","nodeId":"measure"}),
        json!({"width":50,"height":30,"padding":6,"direction":"right","node":"instance__master-inner"}),
        json!({"width":50,"height":30,"direction":"top"}),
        json!({"width":50,"height":30,"direction":"left","page_id":"other","node_id":"other-leaf"}),
        json!({"width":50,"height":30,"direction":"wrong"}),
        json!({"width":50,"height":30,"direction":"right","nodeId":"absent"}),
        json!({"height":30,"pageId":"absent"}),
        json!({"width":"bad"}),
        json!({"width":50,"height":30,"direction":"right","pageId":"absent"}),
    ];
    for preserve in [false, true] {
        state.editor_ui.preserve_authored_geometry = preserve;
        let legacy = find_empty_space_snapshot(&state);
        for value in &values {
            let args = arguments(value.clone());
            let focused = find_empty_space_snapshot_for_call(&state, &args);
            assert!(focused.pages.is_empty());
            assert_eq!(
                focused.call(&args),
                legacy.call(&args),
                "{value}, preserve={preserve}"
            );
        }
    }
    let empty = EditorState::from_document(
        jian_ops_schema::load_str(r#"{"version":"1.0.0","children":[]}"#)
            .unwrap()
            .value,
    );
    let args = arguments(json!({"width":3,"height":4,"direction":"wrong"}));
    assert_eq!(
        find_empty_space_snapshot_for_call(&empty, &args).call(&args),
        find_empty_space_snapshot(&empty).call(&args),
        "empty bounds accept any direction before direction validation"
    );
}

#[test]
fn requested_layout_preserves_duplicate_parent_lookup_and_last_raw_metadata() {
    let state = EditorState::from_document(
        jian_ops_schema::load_str(
            r#"{
      "version":"1.0.0","children":[
       {"id":"same","type":"frame","x":10,"width":100,"height":100,"children":[
        {"id":"a","name":"earlier","type":"rectangle","x":2,"width":3,"height":4}]},
       {"id":"same","type":"frame","x":30,"width":100,"height":100,"children":[
        {"id":"b","type":"rectangle","x":5,"width":6,"height":7}]},
       {"id":"a","name":"last metadata","type":"ellipse","width":8,"height":9}]
    }"#,
        )
        .unwrap()
        .value,
    );
    for value in [
        json!({"parentId":"same","maxDepth":0}),
        json!({"parentId":"a"}),
        json!({"maxDepth":1}),
        json!({"pageId":"0","maxDepth":0}),
    ] {
        let args = arguments(value.clone());
        assert_eq!(
            snapshot_layout_snapshot_for_call(&state, &args).call(&args),
            snapshot_layout_snapshot(&state).call(&args),
            "{value}"
        );
    }
    let args = arguments(json!({"parentId":"same","maxDepth":0}));
    let ToolOutcome::OkJson(response) =
        snapshot_layout_snapshot_for_call(&state, &args).call(&args)
    else {
        panic!()
    };
    let response: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["layout"][0]["name"], "last metadata");
    assert_eq!(response["layout"][0]["type"], "ellipse");
    // Duplicate IDs also share the layout engine's node lookup; preserve its
    // computed coordinate rather than substituting authored positions.
    assert_eq!(response["layout"][1]["x"], 5);
}

#[test]
fn fresh_factories_observe_unmarked_public_mutations_while_trusted_cache_is_explicit() {
    use op_editor_core::pen_node_ext::PenNodeExt;
    let mut state = fixture();
    state.ui.active_page_index = 1;
    let args = arguments(json!({"maxDepth":0}));
    let before = snapshot_layout_snapshot_for_call(&state, &args).call(&args);
    let _ = snapshot_layout_snapshot_cached_for_call(&state, &args).call(&args);
    state.doc.pages.as_mut().unwrap()[1].children[1]
        .base_mut()
        .x = Some(999.0);
    let after = snapshot_layout_snapshot_for_call(&state, &args).call(&args);
    assert_ne!(
        before, after,
        "fresh public factory cannot use unmarked revision-keyed data"
    );
    assert_eq!(after, snapshot_layout_snapshot(&state).call(&args));
    state.mark_document_changed();
    assert_eq!(
        after,
        snapshot_layout_snapshot_cached_for_call(&state, &args).call(&args)
    );
}
