use super::*;
use crate::{command_node::remap_subtree_ids_mapping, EditorCommand, EditorState};

fn reference(children: Option<Vec<PenNode>>) -> PenNode {
    let mut value = serde_json::json!({"type":"ref","id":"instance","ref":"master"});
    if let Some(children) = children {
        value["children"] = serde_json::to_value(children).unwrap();
    }
    serde_json::from_value(value).unwrap()
}

#[test]
fn clone_and_remap_preserve_absent_and_explicit_empty_child_overrides() {
    for source in [
        reference(None),
        reference(Some(vec![])),
        crate::test_support::rect("rectangle", "Rectangle", 1.25, 2.5, 8.0, 6.0),
    ] {
        let mut allocator = SequentialIdAllocator::new(100);
        let copy = deep_clone_with_allocator(&source, &mut allocator, &mut HashSet::new()).unwrap();
        let mut remapped = vec![source.clone()];
        assert!(remap_subtree_ids_mapping(&mut remapped, &mut 100, &mut HashSet::new()).is_some());
        for changed in [copy, remapped.remove(0)] {
            assert_ne!(changed.id_str(), source.id_str());
            let mut expected = serde_json::to_value(&source).unwrap();
            expected["id"] = changed.id_str().into();
            assert_eq!(serde_json::to_value(&changed).unwrap(), expected);
        }
    }
}

#[test]
fn copied_component_instances_keep_inherited_master_content_and_undo_exactly() {
    let document = serde_json::from_value(serde_json::json!({"version":"1.0.0","children":[
        {"type":"frame","id":"master","reusable":true,"width":20,"height":10,"children":[
            {"type":"rectangle","id":"paint","width":20,"height":10,"fill":[{"type":"solid","color":"#ff0000"}]}]},
        {"type":"ref","id":"instance","ref":"master"}
    ]})).unwrap();
    let mut state = EditorState::from_document(document);
    let before = serde_json::to_vec(&state.doc).unwrap();
    assert!(state.apply_undoable(EditorCommand::CopyNode {
        node_id: NodeId::new("instance"),
        target_parent: NodeId::NONE,
        overrides_json: None,
        page_id: None,
    }));
    let copied = state.active_children().last().unwrap();
    assert!(
        copied.children().is_none(),
        "missing children inherit the master"
    );
    let resolved =
        crate::ref_resolve::resolve_refs_for_canvas_roots(std::slice::from_ref(copied), &state.doc);
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].children().unwrap().len(), 1);
    let paint = serde_json::to_value(&resolved[0].children().unwrap()[0]).unwrap();
    assert_eq!(paint["fill"][0]["color"], "#ff0000");
    let after = serde_json::to_vec(&state.doc).unwrap();
    assert!(state.apply(EditorCommand::Undo));
    assert_eq!(serde_json::to_vec(&state.doc).unwrap(), before);
    assert!(state.apply(EditorCommand::Redo));
    assert_eq!(serde_json::to_vec(&state.doc).unwrap(), after);
}

#[test]
fn nested_clone_keeps_all_metadata_and_depth_first_allocator_order() {
    let leaf = reference(None);
    let source = (0..32).fold(leaf, |child, index| {
        crate::test_support::frame(
            &format!("layer-{index}"),
            "Nested",
            index as f64,
            0.0,
            20.0,
            10.0,
            vec![child],
        )
    });
    let mut allocator = SequentialIdAllocator::new(50);
    let clone = deep_clone_with_allocator(&source, &mut allocator, &mut HashSet::new()).unwrap();
    let mut current = &clone;
    let mut original = &source;
    for expected in 50..83 {
        assert_eq!(current.id_str(), format!("n{expected}"));
        let mut value =
            serde_json::to_value(crate::pen_node_ext::clone_without_children(original)).unwrap();
        value["id"] = current.id_str().into();
        value.as_object_mut().unwrap().remove("children");
        let mut copied =
            serde_json::to_value(crate::pen_node_ext::clone_without_children(current)).unwrap();
        copied.as_object_mut().unwrap().remove("children");
        assert_eq!(copied, value);
        if let Some(children) = original.children() {
            original = &children[0];
            current = &current.children().unwrap()[0];
        }
    }
    assert!(current.children().is_none());
    assert_eq!(allocator.next_counter(), 83);
}
