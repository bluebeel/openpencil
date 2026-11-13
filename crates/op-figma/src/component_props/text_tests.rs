use super::*;
use crate::instance::apply_instance_overrides;
use crate::text_mapper::map_figma_text_props;
use crate::tree::TreeNode;
use jian_ops_schema::node::TextContent;

fn object(fields: Vec<(&str, FigValue)>) -> FigValue {
    FigValue::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}
fn guid(id: u32) -> FigValue {
    object(vec![
        ("sessionID", FigValue::Uint(1)),
        ("localID", FigValue::Uint(id)),
    ])
}
fn text(characters: &str) -> FigValue {
    object(vec![("characters", FigValue::Str(characters.into()))])
}
fn assignment(value: FigValue, variable: bool) -> FigValue {
    object(vec![
        ("defID", guid(90)),
        if variable {
            (
                "varValue",
                object(vec![("value", object(vec![("textDataValue", value)]))]),
            )
        } else {
            ("value", object(vec![("textValue", value)]))
        },
    ])
}
fn text_layer(id: u32) -> FigValue {
    object(vec![
        ("guid", guid(id)),
        ("type", FigValue::Str("TEXT".into())),
        ("textData", text("Label")),
        ("textCase", FigValue::Str("UPPER".into())),
        (
            "componentPropRefs",
            FigValue::Array(vec![object(vec![
                ("defID", guid(90)),
                ("componentPropNodeField", FigValue::Str("TEXT_DATA".into())),
            ])]),
        ),
    ])
}
fn instance(assignments: Vec<FigValue>, overrides: Vec<FigValue>) -> FigValue {
    object(vec![
        ("guid", guid(100)),
        ("type", FigValue::Str("INSTANCE".into())),
        ("componentPropAssignments", FigValue::Array(assignments)),
        (
            "symbolData",
            object(vec![
                ("symbolID", guid(1)),
                ("symbolOverrides", FigValue::Array(overrides)),
            ]),
        ),
    ])
}
fn overrides(node: &FigValue) -> &[FigValue] {
    node.get("symbolData")
        .unwrap()
        .get_array("symbolOverrides")
        .unwrap()
}

#[test]
fn component_text_assignment_replaces_master_label_and_preserves_styled_runs() {
    let mut value = text("Preview");
    value.set(
        "characterStyleIDs",
        FigValue::Array(vec![FigValue::Uint(3); 7]),
    );
    value.set(
        "styleOverrideTable",
        FigValue::Array(vec![object(vec![
            ("styleID", FigValue::Uint(3)),
            ("textDecoration", FigValue::Str("UNDERLINE".into())),
        ])]),
    );
    let layer = text_layer(2);
    let master = TreeNode {
        figma: object(vec![("guid", guid(1))]),
        children: vec![TreeNode {
            figma: layer.clone(),
            children: vec![],
        }],
    };
    let mut nodes = vec![
        layer,
        instance(vec![assignment(value.clone(), false)], vec![]),
    ];
    resolve_component_property_swaps(&mut nodes);
    let expanded = apply_instance_overrides(&master, Some(overrides(&nodes[1])), None, None);
    assert_eq!(expanded[0].figma.get("textData"), Some(&value));
    let TextContent::Styled(segments) = map_figma_text_props(&expanded[0].figma).content else {
        panic!("assigned underline must survive");
    };
    assert_eq!(segments[0].text, "PREVIEW");
    assert_eq!(segments[0].underline, Some(true));
    assert_eq!(
        nodes[0].get("textData"),
        Some(&text("Label")),
        "master stays authored"
    );
}

#[test]
fn component_variable_text_preserves_empty_assignments_and_legacy_leaf_precedence() {
    let legacy = object(vec![
        ("guidPath", path_value(vec![guid(2)])),
        ("textData", text("Legacy")),
    ]);
    let mut nodes = vec![
        text_layer(2),
        text_layer(3),
        instance(vec![assignment(text(""), true)], vec![legacy]),
    ];
    resolve_component_property_swaps(&mut nodes);
    let entries = overrides(&nodes[2]);
    assert_eq!(
        entries
            .iter()
            .find(|entry| same_path(entry, &[guid(2)]))
            .unwrap()
            .get("textData"),
        Some(&text("Legacy"))
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| same_path(entry, &[guid(3)]))
            .unwrap()
            .get("textData"),
        Some(&text(""))
    );
}

#[test]
fn nested_component_text_routes_keep_prefix_and_instance_assignment_priority() {
    let nested = object(vec![
        ("guidPath", path_value(vec![guid(7)])),
        (
            "componentPropAssignments",
            FigValue::Array(vec![assignment(text("Nested"), false)]),
        ),
    ]);
    let mut inst = instance(vec![assignment(text("Local"), false)], vec![nested]);
    inst.get_mut("symbolData").unwrap().set(
        "componentPropAssignments",
        FigValue::Array(vec![assignment(text("Inherited"), true)]),
    );
    let mut nodes = vec![text_layer(2), inst];
    resolve_component_property_swaps(&mut nodes);
    let entries = overrides(&nodes[1]);
    assert_eq!(
        entries
            .iter()
            .find(|entry| same_path(entry, &[guid(2)]))
            .unwrap()
            .get("textData"),
        Some(&text("Local"))
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| same_path(entry, &[guid(7), guid(2)]))
            .unwrap()
            .get("textData"),
        Some(&text("Nested"))
    );
}

#[test]
fn missing_text_characters_do_not_erase_an_authored_label() {
    let mut nodes = vec![
        text_layer(2),
        instance(vec![assignment(object(vec![]), false)], vec![]),
    ];
    let original = nodes.clone();
    resolve_component_property_swaps(&mut nodes);
    assert_eq!(nodes, original);
}

#[test]
fn text_assignments_target_only_the_known_master_variant_and_nested_instance() {
    let master = |id| {
        object(vec![
            ("guid", guid(id)),
            ("type", FigValue::Str("SYMBOL".into())),
        ])
    };
    let mut left = text_layer(2);
    left.set("parentIndex", object(vec![("guid", guid(1))]));
    let mut right = text_layer(3);
    right.set("parentIndex", object(vec![("guid", guid(4))]));
    let nested = object(vec![
        ("guid", guid(7)),
        ("type", FigValue::Str("INSTANCE".into())),
        ("symbolData", object(vec![("symbolID", guid(4))])),
    ]);
    let nested_assignment = object(vec![
        ("guidPath", path_value(vec![guid(7)])),
        (
            "componentPropAssignments",
            FigValue::Array(vec![assignment(text("Nested"), false)]),
        ),
    ]);
    let mut nodes = vec![
        master(1),
        master(4),
        left,
        right,
        nested,
        instance(
            vec![assignment(text("Local"), false)],
            vec![nested_assignment],
        ),
    ];
    resolve_component_property_swaps(&mut nodes);
    let entries = overrides(&nodes[5]);
    assert!(entries.iter().any(|entry| same_path(entry, &[guid(2)])));
    assert!(
        !entries.iter().any(|entry| same_path(entry, &[guid(3)])),
        "another variant must not receive this placement's text"
    );
    assert!(entries
        .iter()
        .any(|entry| same_path(entry, &[guid(7), guid(3)])));
    assert!(!entries
        .iter()
        .any(|entry| same_path(entry, &[guid(7), guid(2)])));
}
