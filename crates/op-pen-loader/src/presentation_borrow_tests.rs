use super::*;
use jian_ops_schema::{node::PenNode, sizing::SizingBehavior};
use op_editor_core::EditorState;
use std::borrow::Cow;

fn state(source: &str) -> EditorState {
    EditorState::from_document(jian_ops_schema::load_str(source).unwrap().value)
}

#[test]
fn presentation_noop_keeps_borrowed_and_owned_roots_without_a_new_copy() {
    let state = state(
        r##"{"version":"1.0.0","children":[{"type":"frame","id":"board",
        "width":100,"height":100,"children":[{"type":"rectangle","id":"r",
        "width":10,"height":10,"fill":[{"type":"solid","color":"#123456"}]}]}]}"##,
    );
    assert!(!document_needs_presentation(&state.doc));
    let roots = state.doc.children.as_slice();
    let borrowed = roots_with_presentation(&state, Cow::Borrowed(roots), None, true);
    assert!(matches!(borrowed, Cow::Borrowed(_)));
    assert!(std::ptr::eq(borrowed.as_ptr(), roots.as_ptr()));
    let owned = roots.to_vec();
    let pointer = owned.as_ptr();
    let unchanged = roots_with_presentation(&state, Cow::Owned(owned), None, true);
    assert!(std::ptr::eq(unchanged.as_ptr(), pointer));
    assert_eq!(
        full_scene(&state, true, true),
        full_scene(&state, true, false)
    );
    assert_eq!(
        active_scene(&state, true, true),
        active_scene(&state, true, false)
    );
}

#[test]
fn presentation_binding_keeps_page_scope_and_leaves_the_document_untouched() {
    let state = state(
        r#"{"version":"1.0.0","pages":[
        {"id":"a","name":"First","state":{"fee":{"type":"number","default":1}},
         "children":[]},
        {"id":"b","name":"Second","state":{"fee":{"type":"number","default":8}},
         "children":[{"type":"text","id":"price","content":"fallback",
         "bindings":{"content":"to_str($page.fee)"}}]}]}"#,
    );
    let before = state.doc.clone();
    assert!(document_needs_presentation(&state.doc));
    let roots = state.doc.pages.as_ref().unwrap()[1].children.as_slice();
    let projected = roots_with_presentation(&state, Cow::Borrowed(roots), Some("b"), true);
    assert!(matches!(projected, Cow::Owned(_)));
    let PenNode::Text(text) = &projected[0] else {
        panic!("text projection");
    };
    assert_eq!(
        text.content,
        jian_ops_schema::node::TextContent::Plain("8".into())
    );
    assert_eq!(state.doc, before);
    assert!(matches!(
        roots_with_presentation(&state, Cow::Borrowed(roots), Some("b"), false),
        Cow::Borrowed(_)
    ));
}

#[test]
fn presentation_scroll_expansion_and_preserve_geometry_gate_remain_intact() {
    let mut state = state(
        r#"{"version":"1.0.0","children":[{"type":"frame","id":"screen",
        "screen":"/","layout":"vertical","width":100,"height":20,
        "children":[{"type":"rectangle","id":"body","width":100,"height":60}]}]}"#,
    );
    let before = state.doc.clone();
    let roots = state.doc.children.as_slice();
    assert!(roots_need_presentation(roots));
    let projected = roots_with_presentation(&state, Cow::Borrowed(roots), None, true);
    let PenNode::Frame(frame) = &projected[0] else {
        panic!("screen projection");
    };
    assert!(matches!(frame.container.height, Some(SizingBehavior::Number(h)) if h >= 60.0));
    assert_eq!(state.doc, before);
    state.editor_ui.preserve_authored_geometry = true;
    assert!(matches!(
        roots_with_presentation(&state, Cow::Borrowed(&state.doc.children), None, true),
        Cow::Borrowed(_)
    ));
}
