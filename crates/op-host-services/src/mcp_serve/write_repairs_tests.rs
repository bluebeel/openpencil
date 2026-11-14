use super::*;
use crate::mcp_serve::apply_command_with_undo;

fn state(children: &str) -> EditorState {
    let json = format!(r#"{{"version":"1.0.0","children":{children}}}"#);
    EditorState::from_document(jian_ops_schema::load_str(&json).unwrap().value)
}
fn subtree(json: &str, parent: &str, page_id: Option<&str>) -> C {
    let node = serde_json::from_str(json).unwrap();
    C::InsertAuthoredSubtreePreservingRoots {
        nodes: vec![node],
        parent_id: NodeId::new_opt(parent).unwrap_or(NodeId::NONE),
        page_id: page_id.map(str::to_owned),
    }
}
const AUTHORED: &str = r#"[
  {"type":"frame","id":"screen","width":375,"height":812,"children":[
    {"type":"path","id":"authored-icon","width":14,"height":14,"d":"M6 9l6 6 6-6","stroke":{"thickness":0.54}},
    {"type":"rectangle","id":"authored-slot","width":24,"height":16,"fill":[{"type":"image","url":""}]},
    {"type":"group","id":"absent"},{"type":"group","id":"empty","children":[]}
  ]},
  {"type":"group","id":"last"}
]"#;
const ICON: &str = r#"{"type":"path","id":"new-icon","width":14,"height":14,"d":"M6 9l6 6 6-6"}"#;

#[test]
fn ordinary_edits_and_history_never_repair_unrelated_imports() {
    let mut editor = state(AUTHORED);
    assert!(!editor.editor_ui.preserve_authored_geometry);
    let original = editor.doc.clone();
    let command = C::SetNodeName {
        node_id: NodeId::new("screen"),
        name: "Renamed".into(),
    };
    assert!(apply_command_with_undo(&mut editor, command, true));
    let edited = editor.doc.clone();
    assert_eq!(
        edited.children[0].children(),
        original.children[0].children()
    );
    assert_eq!(edited.children[1], original.children[1]);
    assert!(apply_command_with_undo(&mut editor, C::Undo, true));
    assert_eq!(editor.doc, original);
    assert!(apply_command_with_undo(&mut editor, C::Redo, true));
    assert_eq!(editor.doc, edited);
}

#[test]
fn generated_root_cleanup_preserves_authored_siblings_and_undo() {
    let mut editor = state(AUTHORED);
    let original = editor.doc.clone();
    assert!(apply_command_with_undo(
        &mut editor,
        subtree(ICON, "", None),
        true
    ));
    assert_eq!(&editor.doc.children[..2], &original.children);
    assert!(matches!(editor.doc.children[2], PenNode::IconFont(_)));
    let edited = editor.doc.clone();
    assert!(apply_command_with_undo(&mut editor, C::Undo, true));
    assert_eq!(editor.doc, original);
    assert!(apply_command_with_undo(&mut editor, C::Redo, true));
    assert_eq!(editor.doc, edited);
}

#[test]
fn nested_generated_content_keeps_parent_chrome_and_siblings_exact() {
    let mut editor = state(AUTHORED);
    let original = editor.doc.clone();
    let generated = r#"{"type":"frame","id":"new-content","width":375,"height":"fit_content","children":[{"type":"path","id":"new-icon","width":14,"height":14,"d":"M6 9l6 6 6-6"}]}"#;
    assert!(apply_command_with_undo(
        &mut editor,
        subtree(generated, "screen", None),
        true
    ));
    let children = editor.doc.children[0].children().unwrap();
    assert_eq!(&children[..4], original.children[0].children().unwrap());
    assert_eq!(editor.doc.children[1], original.children[1]);
    let nested = &children[4];
    assert_eq!(
        nested.children().unwrap().len(),
        1,
        "no mobile status bar added to nested content"
    );
    assert_eq!(
        nested.height_px(),
        None,
        "nested fit-content remains authored"
    );
    assert!(matches!(
        nested.children().unwrap()[0],
        PenNode::IconFont(_)
    ));
}

#[test]
fn explicit_refine_repairs_only_its_target() {
    let mut editor = state(AUTHORED);
    let original = editor.doc.clone();
    assert!(apply_command_with_undo(
        &mut editor,
        C::RefineDesign {
            root_id: NodeId::new("authored-icon"),
            canvas_width: None,
            page_id: None,
        },
        true
    ));
    let children = editor.doc.children[0].children().unwrap();
    assert!(matches!(children[0], PenNode::IconFont(_)));
    assert_eq!(
        &children[1..],
        &original.children[0].children().unwrap()[1..]
    );
    assert_eq!(editor.doc.children[1], original.children[1]);
}

#[test]
fn explicit_page_cleanup_restores_active_page_and_other_pages() {
    let json = format!(
        r#"{{"version":"1.0.0","children":[],"pages":[{{"id":"one","name":"One","children":{AUTHORED}}},{{"id":"two","name":"Two","children":[]}}]}}"#
    );
    let mut editor = EditorState::from_document(jian_ops_schema::load_str(&json).unwrap().value);
    let original_page = editor.doc.pages.as_ref().unwrap()[0].clone();
    assert!(apply_command_with_undo(
        &mut editor,
        subtree(ICON, "", Some("two")),
        true
    ));
    assert_eq!(editor.ui.active_page_index, 0);
    let pages = editor.doc.pages.as_ref().unwrap();
    assert_eq!(pages[0], original_page);
    assert!(matches!(pages[1].children[0], PenNode::IconFont(_)));
}

#[test]
fn atomic_batch_repairs_new_ancestor_once_and_retains_child_order() {
    let mut editor = state(AUTHORED);
    let original = editor.doc.clone();
    let root = r#"{"type":"group","id":"new-group","children":[]}"#;
    let commands = vec![subtree(root, "", None), subtree(ICON, "new-group", None)];
    assert!(apply_command_with_undo(
        &mut editor,
        C::Batch { commands },
        true
    ));
    assert_eq!(&editor.doc.children[..2], &original.children);
    let generated = editor.doc.children[2].children().unwrap();
    assert_eq!(generated.len(), 1);
    assert!(matches!(generated[0], PenNode::IconFont(_)));
}

#[test]
fn rejected_batch_and_preserve_mode_skip_all_cleanup() {
    let mut editor = state(AUTHORED);
    let original = editor.doc.clone();
    let commands = vec![
        subtree(ICON, "", None),
        C::DeleteNode {
            node_id: NodeId::new("missing"),
            page_id: None,
        },
    ];
    assert!(!apply_command_with_undo(
        &mut editor,
        C::Batch { commands },
        true
    ));
    assert_eq!(editor.doc, original);
    editor.editor_ui.preserve_authored_geometry = true;
    assert!(apply_command_with_undo(
        &mut editor,
        subtree(ICON, "", None),
        true
    ));
    assert!(matches!(editor.doc.children[2], PenNode::Path(_)));
    assert_eq!(&editor.doc.children[..2], &original.children);
}

#[test]
fn only_explicit_generation_tools_enable_cleanup() {
    for name in [
        "batch_design",
        "design_skeleton",
        "design_content",
        "design_refine",
    ] {
        assert!(generated_tool(name));
    }
    for name in [
        "insert_node",
        "update_node",
        "import_svg",
        "copy_node",
        "undo",
        "redo",
        "set_node_name",
    ] {
        assert!(!generated_tool(name));
    }
}

#[test]
fn append_only_capture_retains_a_prefix_length_instead_of_old_sibling_ids() {
    let mut editor = state(AUTHORED);
    let command = subtree(ICON, "", None);
    let plan = RepairPlan::capture(&editor, &command, true);
    assert!(plan.append_only);
    assert!(matches!(
        plan.insertions[0].before,
        BeforeInsertion::Append(2)
    ));
    let before = editor.document_revision();
    assert!(editor.apply(command));
    plan.apply(&mut editor, before);
    assert!(matches!(editor.doc.children[2], PenNode::IconFont(_)));
}

#[test]
fn root_replacement_and_mixed_batches_keep_identifier_difference_capture() {
    let editor = state(AUTHORED);
    let node = serde_json::from_str(ICON).unwrap();
    let replacement = C::InsertAuthoredSubtree {
        nodes: vec![node],
        parent_id: NodeId::NONE,
        page_id: None,
    };
    let plan = RepairPlan::capture(&editor, &replacement, true);
    assert!(!plan.append_only);
    assert!(matches!(
        plan.insertions[0].before,
        BeforeInsertion::Identifiers(_)
    ));
    let mixed = C::Batch {
        commands: vec![
            subtree(ICON, "", None),
            C::SetNodeName {
                node_id: NodeId::new("screen"),
                name: "changed".into(),
            },
        ],
    };
    let plan = RepairPlan::capture(&editor, &mixed, true);
    assert!(!plan.append_only);
    assert!(matches!(
        plan.insertions[0].before,
        BeforeInsertion::Identifiers(_)
    ));
}

#[test]
fn direct_append_paths_produce_the_exact_legacy_cleanup_document() {
    let mut by_ids = state(AUTHORED);
    assert!(by_ids.apply(subtree(ICON, "screen", None)));
    assert!(by_ids.apply(subtree(
        r#"{"type":"path","id":"other-new-icon","width":14,"height":14,"d":"M6 9l6 6 6-6"}"#,
        "",
        None
    )));
    let mut by_paths = by_ids.clone();
    let ids = HashSet::from(["new-icon".to_string(), "other-new-icon".to_string()]);
    let expected = repair_selected(&mut by_ids, &ids);
    let actual = repair_paths(&mut by_paths, vec![vec![0, 4], vec![2]]);
    assert_eq!(actual, expected);
    assert_eq!(by_paths.doc, by_ids.doc);
    assert_eq!(by_paths.ui.active_page_index, by_ids.ui.active_page_index);
    assert!(matches!(by_paths.doc.children[2], PenNode::IconFont(_)));
}

#[test]
fn parent_path_resolution_matches_depth_first_command_lookup_order() {
    let editor = state(
        r#"[
        {"type":"group","id":"outer","children":[{"type":"group","id":"parent","children":[]}]},
        {"type":"group","id":"parent","children":[]}
    ]"#,
    );
    assert_eq!(
        insertion_parent_path(&editor.doc.children, &NodeId::new("parent")),
        Some(vec![0, 0])
    );
    assert_eq!(
        insertion_parent_path(&editor.doc.children, &NodeId::NONE),
        Some(vec![])
    );
    assert_eq!(
        insertion_parent_path(&editor.doc.children, &NodeId::new("missing")),
        None
    );
}

#[test]
fn refine_duplicate_id_skips_ambiguous_heuristic_target() {
    let mut editor = state(&format!("[{ICON},{ICON}]"));
    let second = editor.doc.children[1].clone();
    assert!(apply_command_with_undo(
        &mut editor,
        C::RefineDesign {
            root_id: NodeId::new("new-icon"),
            canvas_width: None,
            page_id: None,
        },
        true
    ));
    assert_eq!(editor.doc.children[1], second);
}
