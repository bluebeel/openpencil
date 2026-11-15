use super::*;

fn state(value: serde_json::Value) -> EditorState {
    EditorState::from_document(jian_ops_schema::load_str(&value.to_string()).unwrap().value)
}

#[test]
fn bounded_builder_accepts_plain_spine_and_refuses_unsupported_scopes() {
    let mut s = state(serde_json::json!({"version":"1.0.0","children":[
        {"id":"root","type":"frame","width":100,"height":80,"children":[
            {"id":"target","type":"frame","width":20,"height":20,"children":[
                {"id":"leaf","type":"rectangle","width":3,"height":4}]}]}]}));
    assert!(try_geometry(&s, 0, "target").is_some());
    assert!(try_geometry(&s, 0, "missing").is_none());
    assert!(try_geometry(&s, 0, "virtual__child").is_none());
    assert!(try_geometry(&s, 9, "target").is_none());
    s.editor_ui.workspace.visible = true;
    assert!(try_geometry(&s, 0, "target").is_none());
    s.editor_ui.workspace.visible = false;
    let duplicate = s.doc.children[0].clone();
    s.doc.children.push(duplicate);
    assert!(try_geometry(&s, 0, "target").is_none());
}

#[test]
fn bounded_builder_refuses_large_target_clone_and_index_sensitive_widget_ancestor() {
    let children: Vec<_> = (0..MAX_TARGET_NODES)
        .map(|i| {
            serde_json::json!({
        "id":format!("c{i}"),"type":"rectangle","width":1,"height":1})
        })
        .collect();
    let s = state(serde_json::json!({"version":"1.0.0","children":[
        {"id":"target","type":"frame","width":100,"height":80,"children":children}]}));
    assert!(try_geometry(&s, 0, "target").is_none());
    let s = state(serde_json::json!({"version":"1.0.0","children":[
        {"id":"tabs","type":"tabs","width":100,"height":80,"children":[
            {"id":"target","type":"frame","width":20,"height":20}]}]}));
    assert!(try_geometry(&s, 0, "target").is_none());
}
