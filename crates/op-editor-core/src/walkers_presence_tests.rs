//! Tree searches preserve schema None/Some(empty); only insertion authors children.

use super::*;
use crate::{command_node::replace_node_in_children, EditorCommand, EditorState};
use serde_json::json;

fn node(value: serde_json::Value) -> PenNode {
    serde_json::from_value(value).unwrap()
}

fn leaf(id: &str) -> PenNode {
    node(
        json!({"type":"rectangle","id":id,"x":-0.0,"y":0.46000000834465027,
        "width":11.899999618530273,"height":8}),
    )
}

fn forest() -> Vec<PenNode> {
    let mut nodes: Vec<_> = ["frame", "group", "rectangle", "tabs", "ref"]
        .into_iter()
        .map(|kind| {
            let mut value = json!({"type":kind,"id":format!("absent-{kind}")});
            if kind == "ref" {
                value["ref"] = "master".into();
            }
            node(value)
        })
        .collect();
    nodes.push(node(
        json!({"type":"frame","id":"explicit-empty","children":[]}),
    ));
    nodes.push(node(json!({"type":"frame","id":"parent","children":[
        {"type":"frame","id":"nested-empty"},
        {"type":"rectangle","id":"target","width":12,"height":8},
        {"type":"rectangle","id":"last","width":4,"height":2}
    ]})));
    nodes
}

fn canonical(nodes: &[PenNode]) -> Vec<u8> {
    serde_json::to_vec(nodes).unwrap()
}

fn children(nodes: &mut [PenNode]) -> &mut Vec<PenNode> {
    nodes.last_mut().unwrap().children_mut().unwrap()
}

#[test]
fn missing_recursive_targets_do_not_author_children_arrays() {
    let before = forest();
    let missing = NodeId::new("missing");
    for operation in 0..9 {
        let mut nodes = before.clone();
        match operation {
            0 => assert!(find_node_mut(&mut nodes, &missing).is_none()),
            1 => assert!(!remove_from_children(&mut nodes, &missing)),
            2 => assert!(extract_node(&mut nodes, &missing).is_none()),
            3 => assert!(insert_before_in_children(&mut nodes, &missing, leaf("new")).is_err()),
            4 => assert!(insert_after_in_children(&mut nodes, &missing, leaf("new")).is_err()),
            5 => assert!(append_into(&mut nodes, &missing, leaf("new")).is_err()),
            6 => assert!(prepend_into(&mut nodes, &missing, leaf("new")).is_err()),
            7 => assert!(!reorder_in_children(
                &mut nodes,
                &missing,
                ReorderDirection::Up
            )),
            8 => {
                let mut replacement = Some(leaf("new"));
                assert!(!replace_node_in_children(
                    &mut nodes,
                    &missing,
                    &mut replacement
                ));
                assert!(replacement.is_some());
            }
            _ => unreachable!(),
        }
        assert_eq!(
            canonical(&nodes),
            canonical(&before),
            "operation {operation}"
        );
    }
}

#[test]
fn found_recursive_targets_only_mutate_their_intended_sibling_slot() {
    let before = forest();
    let target = NodeId::new("target");
    for operation in 0..7 {
        let mut nodes = before.clone();
        let mut expected = before.clone();
        match operation {
            0 => {
                assert!(remove_from_children(&mut nodes, &target));
                children(&mut expected).remove(1);
            }
            1 => {
                let extracted = extract_node(&mut nodes, &target).unwrap();
                assert_eq!(extracted, children(&mut expected).remove(1));
            }
            2 => {
                assert!(insert_before_in_children(&mut nodes, &target, leaf("new")).is_ok());
                children(&mut expected).insert(1, leaf("new"));
            }
            3 => {
                assert!(insert_after_in_children(&mut nodes, &target, leaf("new")).is_ok());
                children(&mut expected).insert(2, leaf("new"));
            }
            4 => {
                assert!(reorder_in_children(
                    &mut nodes,
                    &target,
                    ReorderDirection::Up
                ));
                children(&mut expected).swap(0, 1);
            }
            5 => {
                assert!(reorder_in_children(
                    &mut nodes,
                    &target,
                    ReorderDirection::Down
                ));
                children(&mut expected).swap(1, 2);
            }
            6 => {
                let mut replacement = Some(leaf("replacement"));
                assert!(replace_node_in_children(
                    &mut nodes,
                    &target,
                    &mut replacement
                ));
                assert!(replacement.is_none());
                children(&mut expected)[1] = leaf("replacement");
            }
            _ => unreachable!(),
        }
        assert_eq!(
            canonical(&nodes),
            canonical(&expected),
            "operation {operation}"
        );
    }
}

#[test]
fn explicit_insertion_still_initializes_only_the_requested_parent() {
    for prepend in [false, true] {
        let mut nodes = forest();
        let mut expected = nodes.clone();
        let target = NodeId::new("nested-empty");
        let result = if prepend {
            prepend_into(&mut nodes, &target, leaf("new"))
        } else {
            append_into(&mut nodes, &target, leaf("new"))
        };
        assert!(result.is_ok());
        *children(&mut expected)[0].children_mut().unwrap() = vec![leaf("new")];
        assert_eq!(canonical(&nodes), canonical(&expected));
    }
}

#[test]
fn rich_patch_copy_delete_and_undo_keep_all_unrelated_child_presence_and_bits() {
    let document = serde_json::from_value(json!({
        "version": "1.0.0", "children": forest()
    }))
    .unwrap();
    let mut state = EditorState::from_document(document);
    let before = serde_json::to_vec(&state.doc).unwrap();
    let patch = json!({"fill":[{"type":"solid","color":"#2563eb","opacity":1.0}]}).to_string();
    assert!(state.apply_undoable(EditorCommand::PatchNodeData {
        node_id: NodeId::new("target"),
        patch_json: patch,
        page_id: None,
    }));
    let after_patch = serde_json::to_vec(&state.doc).unwrap();
    for original in &forest()[..6] {
        let actual = find_node(state.active_children(), &NodeId::new(original.id_str())).unwrap();
        assert_eq!(
            serde_json::to_vec(actual).unwrap(),
            serde_json::to_vec(original).unwrap()
        );
    }
    assert!(state.apply_undoable(EditorCommand::CopyNode {
        node_id: NodeId::new("absent-frame"),
        target_parent: NodeId::new("parent"),
        overrides_json: None,
        page_id: None,
    }));
    let copied = find_node(state.active_children(), &NodeId::new("parent"))
        .unwrap()
        .children()
        .unwrap()
        .last()
        .unwrap()
        .id_str()
        .to_string();
    assert!(state.apply_undoable(EditorCommand::DeleteNode {
        node_id: NodeId::new(&copied),
        page_id: None,
    }));
    assert_eq!(serde_json::to_vec(&state.doc).unwrap(), after_patch);
    for _ in 0..3 {
        assert!(state.apply(EditorCommand::Undo));
    }
    assert_eq!(serde_json::to_vec(&state.doc).unwrap(), before);
    for _ in 0..3 {
        assert!(state.apply(EditorCommand::Redo));
    }
    assert_eq!(serde_json::to_vec(&state.doc).unwrap(), after_patch);
}
