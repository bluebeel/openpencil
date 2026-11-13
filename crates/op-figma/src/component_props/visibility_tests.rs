use super::*;
use crate::instance::apply_instance_overrides;
use crate::tree::TreeNode;

fn obj(fields: Vec<(&str, FigValue)>) -> FigValue {
    FigValue::Object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

fn guid(id: u32) -> FigValue {
    obj(vec![
        ("sessionID", FigValue::Uint(1)),
        ("localID", FigValue::Uint(id)),
    ])
}

fn visibility_layer(id: u32, visible: bool) -> FigValue {
    obj(vec![
        ("guid", guid(id)),
        ("type", FigValue::Str("FRAME".into())),
        ("visible", FigValue::Bool(visible)),
        (
            "componentPropRefs",
            FigValue::Array(vec![obj(vec![
                ("defID", guid(90)),
                ("componentPropNodeField", FigValue::Str("VISIBLE".into())),
            ])]),
        ),
    ])
}

fn assignment(visible: bool, variable_encoding: bool) -> FigValue {
    let value = obj(vec![("boolValue", FigValue::Bool(visible))]);
    obj(vec![
        ("defID", guid(90)),
        if variable_encoding {
            ("varValue", obj(vec![("value", value)]))
        } else {
            ("value", value)
        },
    ])
}

fn instance(assignments: Vec<FigValue>, overrides: Vec<FigValue>) -> FigValue {
    obj(vec![
        ("guid", guid(100)),
        ("type", FigValue::Str("INSTANCE".into())),
        ("componentPropAssignments", FigValue::Array(assignments)),
        (
            "symbolData",
            obj(vec![
                ("symbolID", guid(1)),
                ("symbolOverrides", FigValue::Array(overrides)),
            ]),
        ),
    ])
}

fn overrides(instance: &FigValue) -> &[FigValue] {
    instance
        .get("symbolData")
        .unwrap()
        .get_array("symbolOverrides")
        .unwrap()
}

#[test]
fn explicit_false_hides_all_referenced_layers_after_instance_expansion() {
    for variable_encoding in [false, true] {
        let left = visibility_layer(2, true);
        let right = visibility_layer(3, true);
        let master = TreeNode {
            figma: obj(vec![
                ("guid", guid(1)),
                ("type", FigValue::Str("SYMBOL".into())),
            ]),
            children: vec![
                TreeNode {
                    figma: left.clone(),
                    children: vec![],
                },
                TreeNode {
                    figma: right.clone(),
                    children: vec![],
                },
            ],
        };
        let mut nodes = vec![
            left,
            right,
            instance(vec![assignment(false, variable_encoding)], vec![]),
        ];
        resolve_component_property_swaps(&mut nodes);

        let expanded = apply_instance_overrides(&master, Some(overrides(&nodes[2])), None, None);

        assert_eq!(expanded.len(), 2);
        assert!(expanded
            .iter()
            .all(|layer| layer.figma.get_bool("visible") == Some(false)));
    }
}

#[test]
fn instance_assignment_overrides_inherited_assignment_and_master_visibility() {
    for visible in [false, true] {
        let mut layer = visibility_layer(2, !visible);
        let mut inst = instance(vec![assignment(visible, false)], vec![]);
        inst.get_mut("symbolData").unwrap().set(
            "componentPropAssignments",
            FigValue::Array(vec![assignment(!visible, true)]),
        );
        // Unassigned masters retain their authored visibility.
        let original = layer.clone();
        let mut nodes = vec![layer.clone(), inst];
        resolve_component_property_swaps(&mut nodes);
        assert_eq!(nodes[0].get_bool("visible"), original.get_bool("visible"));
        let master = TreeNode {
            figma: obj(vec![("guid", guid(1))]),
            children: vec![TreeNode {
                figma: std::mem::replace(&mut layer, FigValue::Null),
                children: vec![],
            }],
        };
        let expanded = apply_instance_overrides(&master, Some(overrides(&nodes[1])), None, None);
        assert_eq!(expanded[0].figma.get_bool("visible"), Some(visible));
    }
}

#[test]
fn nested_assignment_routes_keep_the_instance_prefix_and_explicit_leaf_override() {
    let nested = obj(vec![
        ("guidPath", path_value(vec![guid(7)])),
        (
            "componentPropAssignments",
            FigValue::Array(vec![assignment(false, false)]),
        ),
    ]);
    let leaf = obj(vec![
        ("guidPath", path_value(vec![guid(7), guid(2)])),
        ("visible", FigValue::Bool(true)),
        ("name", FigValue::Str("locally shown".into())),
    ]);
    let mut nodes = vec![
        visibility_layer(2, true),
        visibility_layer(3, true),
        instance(vec![], vec![nested, leaf]),
    ];
    resolve_component_property_swaps(&mut nodes);
    let entries = overrides(&nodes[2]);
    let explicit = entries
        .iter()
        .find(|e| same_path(e, &[guid(7), guid(2)]))
        .unwrap();
    let assigned = entries
        .iter()
        .find(|e| same_path(e, &[guid(7), guid(3)]))
        .unwrap();
    assert_eq!(explicit.get_bool("visible"), Some(true));
    assert_eq!(explicit.get_str("name"), Some("locally shown"));
    assert_eq!(assigned.get_bool("visible"), Some(false));
    assert!(!entries.iter().any(|e| same_path(e, &[guid(3)])));
}

#[test]
fn self_referenced_boolean_false_is_not_lost_to_an_authored_true() {
    let mut layer = visibility_layer(2, true);
    layer.set(
        "componentPropAssignments",
        FigValue::Array(vec![assignment(false, false)]),
    );
    let mut nodes = vec![layer];
    resolve_component_property_swaps(&mut nodes);
    assert_eq!(nodes[0].get_bool("visible"), Some(false));
}
