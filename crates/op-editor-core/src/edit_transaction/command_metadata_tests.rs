use super::{apply, bytes, state};
use crate::{EditorCommand as C, NodeId};

#[test]
fn scoped_header_scalar_families_match_full_and_restore_absent_maps() {
    let mut scoped = state();
    let mut full = scoped.clone();
    let before = bytes(&scoped);
    let mut commands = Vec::new();
    for (name, kind, initial, scalar) in [
        ("color", "color", "#123456", None),
        (
            "number",
            "number",
            "-0.0",
            Some(crate::VariableScalarPayload::Number(11.899999618530273)),
        ),
        (
            "text",
            "string",
            "before",
            Some(crate::VariableScalarPayload::String("after".into())),
        ),
        (
            "bool",
            "boolean",
            "false",
            Some(crate::VariableScalarPayload::Boolean(true)),
        ),
    ] {
        commands.push(C::CreateVariable {
            name: name.into(),
            kind: kind.into(),
            default_value: initial.into(),
        });
        commands.push(match scalar {
            Some(scalar) => C::SetVariableScalar {
                name: name.into(),
                scalar,
            },
            None => C::SetVariableColor {
                name: name.into(),
                hex: "#abcdef".into(),
            },
        });
    }
    for command in commands {
        assert!(full.apply_undoable(command.clone()));
        apply(&mut scoped, command);
        assert_eq!(bytes(&scoped), bytes(&full));
    }
    while full.history.can_undo() {
        assert!(full.undo());
        assert!(scoped.undo());
        assert_eq!(bytes(&scoped), bytes(&full));
    }
    assert_eq!(bytes(&scoped), before);
    assert!(scoped.doc.variables.is_none());
    while full.history.can_redo() {
        assert!(full.redo());
        assert!(scoped.redo());
        assert_eq!(bytes(&scoped), bytes(&full));
    }
}

#[test]
fn scoped_header_theme_state_and_source_ledger_batches_match_full() {
    let mut scoped = state();
    let mut full = scoped.clone();
    let state_entry = serde_json::from_str(r#"{"type":"number","default":-0.0}"#).unwrap();
    let variables =
        serde_json::from_str(r#"{"n":{"type":"number","value":11.899999618530273}}"#).unwrap();
    let command = C::Batch {
        commands: vec![
            C::SetThemes {
                themes: std::collections::BTreeMap::from([(
                    "mode".into(),
                    vec!["light".into(), "dark".into()],
                )]),
                replace: true,
            },
            C::MergeAppState {
                plan_idx: 7,
                state: std::collections::BTreeMap::from([("count".into(), state_entry)]),
            },
            C::UpsertVariables {
                variables,
                key: "tokens".into(),
                source_path: Some("src/tokens".into()),
                source_hash: Some("hash".into()),
            },
        ],
    };
    let before = bytes(&scoped);
    assert!(full.apply_undoable(command.clone()));
    apply(&mut scoped, command);
    let after = bytes(&scoped);
    assert_eq!(after, bytes(&full));
    assert!(scoped.undo());
    assert!(full.undo());
    assert_eq!(bytes(&scoped), before);
    assert_eq!(bytes(&scoped), bytes(&full));
    assert!(scoped.redo());
    assert_eq!(bytes(&scoped), after);
}

#[test]
fn scoped_header_rename_delete_cover_references_without_authoring_children() {
    let mut scoped = state();
    scoped.doc.variables =
        Some(serde_json::from_str(r##"{"tone":{"type":"color","value":"#123456"}}"##).unwrap());
    let node = crate::walkers::find_node_mut(
        &mut scoped.doc.pages.as_mut().unwrap()[1].children,
        &NodeId::new("b"),
    )
    .unwrap();
    let jian_ops_schema::node::PenNode::Text(text) = node else {
        panic!()
    };
    text.content = jian_ops_schema::node::TextContent::Plain("$tone".into());
    let mut full = scoped.clone();
    let before = bytes(&scoped);
    for command in [
        C::RenameVariable {
            old_name: "tone".into(),
            new_name: "renamed".into(),
        },
        C::DeleteVariable {
            name: "renamed".into(),
        },
    ] {
        assert!(full.apply_undoable(command.clone()));
        apply(&mut scoped, command);
        assert_eq!(bytes(&scoped), bytes(&full));
    }
    let after = bytes(&scoped);
    assert!(!String::from_utf8_lossy(&after).contains("\"ref\":\"root-a\",\"children\":[]"));
    for _ in 0..2 {
        assert!(scoped.undo());
        assert!(full.undo());
        assert_eq!(bytes(&scoped), bytes(&full));
    }
    assert_eq!(bytes(&scoped), before);
    for _ in 0..2 {
        assert!(scoped.redo());
    }
    assert_eq!(bytes(&scoped), after);
}

#[test]
fn refused_page_and_empty_history_do_not_capture_and_preserve_redo() {
    let mut state = state();
    let before = bytes(&state);
    for command in [
        C::Undo,
        C::Redo,
        C::DeleteVariable {
            name: "missing".into(),
        },
        C::SetVariableScalar {
            name: "missing".into(),
            scalar: crate::VariableScalarPayload::Number(2.0),
        },
        C::SetVariableColor {
            name: "missing".into(),
            hex: "#123456".into(),
        },
        C::CreateVariable {
            name: " ".into(),
            kind: "number".into(),
            default_value: "0".into(),
        },
        C::SetActivePage { index: u32::MAX },
        C::Batch { commands: vec![] },
        C::UpdateNode {
            node_id: NodeId::new("a"),
            x: Some(9),
            y: None,
            width: None,
            height: None,
            name: None,
            fill_hex: None,
            page_id: Some("missing".into()),
        },
    ] {
        let mut capture = state.begin_command_edit(&command);
        assert!(matches!(
            capture.kind,
            super::super::CaptureKind::Refused { .. }
        ));
        assert!(!state
            .apply_command_edit_then(command, &mut capture, |_| panic!("refused maintenance")));
        state.abort_command_edit(capture).unwrap();
        assert_eq!(bytes(&state), before);
    }
    apply(
        &mut state,
        C::UpdateNode {
            node_id: NodeId::new("a"),
            x: Some(7),
            y: None,
            width: None,
            height: None,
            name: None,
            fill_hex: None,
            page_id: None,
        },
    );
    assert!(state.undo());
    let history = state.history.clone();
    let command = C::SetActivePage { index: u32::MAX };
    let mut capture = state.begin_command_edit(&command);
    assert!(matches!(
        capture.kind,
        super::super::CaptureKind::Refused { .. }
    ));
    assert!(
        !state.apply_command_edit_then(command, &mut capture, |_| panic!("refused maintenance"))
    );
    assert_eq!(bytes(&state), before);
    assert_eq!(state.history.past, history.past);
    assert_eq!(state.history.future, history.future);
}

#[test]
fn unknown_scalar_setters_preserve_absent_variables_on_failure() {
    let mut state = state();
    let before = bytes(&state);
    assert!(!state.set_variable_number("missing", 2.0));
    assert!(!state.set_variable_string("missing", "value"));
    assert!(!state.set_variable_boolean("missing", true));
    assert!(!state.set_variable_color("missing", "#123456"));
    assert_eq!(bytes(&state), before);
}

#[test]
fn scoped_header_failed_batch_and_external_commit_restore_exact_fields() {
    let mut state = state();
    let before = bytes(&state);
    let create = C::CreateVariable {
        name: "added".into(),
        kind: "number".into(),
        default_value: "-0.0".into(),
    };
    let command = C::Batch {
        commands: vec![
            create.clone(),
            C::UpdateNode {
                node_id: NodeId::new("a"),
                x: None,
                y: None,
                width: Some(-1),
                height: None,
                name: None,
                fill_hex: None,
                page_id: None,
            },
        ],
    };
    let mut capture = state.begin_command_edit(&command);
    assert!(capture.is_scoped());
    assert!(
        !state.apply_command_edit_then(command, &mut capture, |_| panic!(
            "failed Batch must not maintain"
        ))
    );
    assert_eq!(bytes(&state), before);
    assert!(state.history.past.is_empty());
    let mut capture = state.begin_command_edit(&create);
    assert!(state.apply_command_edit_then(create, &mut capture, |_| {}));
    assert!(state.doc.variables.as_ref().unwrap().contains_key("added"));
    state.abort_command_edit(capture).unwrap();
    assert_eq!(bytes(&state), before);
    assert!(state.history.past.is_empty());
}

#[test]
fn scoped_header_rejects_unversioned_metadata_changes_before_apply() {
    let mut state = state();
    let command = C::CreateVariable {
        name: "added".into(),
        kind: "number".into(),
        default_value: "0".into(),
    };
    let mut capture = state.begin_command_edit(&command);
    let revision = state.document_revision();
    state.doc.name = Some("public unversioned write".into());
    let current = bytes(&state);
    assert_eq!(state.document_revision(), revision);
    assert!(
        !state.apply_command_edit_then(command, &mut capture, |_| panic!(
            "stale metadata must not maintain"
        ))
    );
    assert_eq!(bytes(&state), current);
    assert!(state.doc.variables.is_none());
    assert!(state.history.past.is_empty());
}
