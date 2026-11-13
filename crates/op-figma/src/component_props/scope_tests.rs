use super::*;

fn obj(fields: Vec<(&str, FigValue)>) -> FigValue {
    FigValue::Object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn guid(id: u32) -> FigValue {
    obj(vec![
        ("sessionID", FigValue::Uint(1)),
        ("localID", FigValue::Uint(id)),
    ])
}
fn master(id: u32) -> FigValue {
    obj(vec![
        ("guid", guid(id)),
        ("type", FigValue::Str("SYMBOL".into())),
    ])
}
fn owner(id: u32, parent: Option<u32>, field: &str) -> FigValue {
    let mut node = obj(vec![
        ("guid", guid(id)),
        (
            "componentPropRefs",
            FigValue::Array(vec![obj(vec![
                ("defID", guid(777)),
                ("componentPropNodeField", FigValue::Str(field.into())),
            ])]),
        ),
    ]);
    if let Some(parent) = parent {
        node.set("parentIndex", obj(vec![("guid", guid(parent))]));
    }
    node
}
fn assignment(field: &str) -> FigValue {
    let value = if field == "VISIBLE" {
        obj(vec![("boolValue", FigValue::Bool(false))])
    } else {
        obj(vec![("guidValue", guid(300))])
    };
    obj(vec![("defID", guid(777)), ("value", value)])
}
fn instance(target: u32, field: &str) -> FigValue {
    obj(vec![
        ("guid", guid(100)),
        (
            "symbolData",
            obj(vec![
                ("symbolID", guid(target)),
                ("symbolOverrides", FigValue::Array(vec![])),
            ]),
        ),
        (
            "componentPropAssignments",
            FigValue::Array(vec![assignment(field)]),
        ),
    ])
}

#[test]
fn component_scope_visibility_and_swap_target_only_actual_variant_master() {
    for field in ["VISIBLE", "INSTANCE_SWAP"] {
        for swapped in [false, true] {
            let mut placement = instance(1, field);
            if swapped {
                placement.set("overriddenSymbolID", guid(2));
            }
            let mut nodes = vec![
                master(1),
                master(2),
                owner(11, Some(1), field),
                owner(22, Some(2), field),
                placement,
            ];
            resolve_component_property_swaps(&mut nodes);
            let overrides = nodes[4]
                .get("symbolData")
                .unwrap()
                .get_array("symbolOverrides")
                .unwrap();
            assert_eq!(
                overrides.len(),
                1,
                "shared definition must not fan out across variants"
            );
            assert!(same_path(
                &overrides[0],
                &[guid(if swapped { 22 } else { 11 })]
            ));
        }
    }
}

#[test]
fn component_scope_nested_override_resolves_selected_inner_master() {
    for field in ["VISIBLE", "INSTANCE_SWAP"] {
        let nested = obj(vec![
            ("guid", guid(200)),
            ("parentIndex", obj(vec![("guid", guid(1))])),
            ("symbolData", obj(vec![("symbolID", guid(2))])),
        ]);
        let placement = obj(vec![
            ("guid", guid(100)),
            (
                "symbolData",
                obj(vec![
                    ("symbolID", guid(1)),
                    (
                        "symbolOverrides",
                        FigValue::Array(vec![obj(vec![
                            ("guidPath", path_value(vec![guid(200)])),
                            (
                                "componentPropAssignments",
                                FigValue::Array(vec![assignment(field)]),
                            ),
                        ])]),
                    ),
                ]),
            ),
        ]);
        let mut nodes = vec![
            master(1),
            master(2),
            owner(11, Some(1), field),
            owner(22, Some(2), field),
            nested,
            placement,
        ];
        resolve_component_property_swaps(&mut nodes);
        let overrides = nodes[5]
            .get("symbolData")
            .unwrap()
            .get_array("symbolOverrides")
            .unwrap();
        assert_eq!(
            overrides.len(),
            2,
            "authored entry plus one reachable inner route"
        );
        assert!(overrides
            .iter()
            .any(|o| same_path(o, &[guid(200), guid(22)])));
        assert!(!overrides
            .iter()
            .any(|o| same_path(o, &[guid(200), guid(11)])));
    }
}

#[test]
fn component_scope_unknown_owner_ancestry_preserves_existing_routes() {
    let mut nodes = vec![
        master(1),
        owner(11, None, "VISIBLE"),
        instance(1, "VISIBLE"),
    ];
    resolve_component_property_swaps(&mut nodes);
    let overrides = nodes[2]
        .get("symbolData")
        .unwrap()
        .get_array("symbolOverrides")
        .unwrap();
    assert_eq!(
        overrides.len(),
        1,
        "partial source ancestry is not proof of exclusion"
    );
}
