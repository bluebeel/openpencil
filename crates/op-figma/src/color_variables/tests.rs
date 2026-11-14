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
fn color(r: f32, g: f32, b: f32, a: f32) -> FigValue {
    obj(vec![
        ("r", FigValue::Float(r)),
        ("g", FigValue::Float(g)),
        ("b", FigValue::Float(b)),
        ("a", FigValue::Float(a)),
    ])
}
fn literal(value: FigValue) -> FigValue {
    obj(vec![("value", obj(vec![("colorValue", value)]))])
}
fn alias(id: u32) -> FigValue {
    obj(vec![(
        "value",
        obj(vec![("alias", obj(vec![("guid", guid(id))]))]),
    )])
}
fn variable(id: u32, collection: u32, values: Vec<(u32, FigValue)>) -> FigValue {
    obj(vec![
        ("guid", guid(id)),
        ("type", FigValue::Str("VARIABLE".into())),
        ("variableResolvedType", FigValue::Str("COLOR".into())),
        ("variableSetID", obj(vec![("guid", guid(collection))])),
        (
            "variableDataValues",
            obj(vec![(
                "entries",
                FigValue::Array(
                    values
                        .into_iter()
                        .map(|(mode, data)| {
                            obj(vec![("modeID", guid(mode)), ("variableData", data)])
                        })
                        .collect(),
                ),
            )]),
        ),
    ])
}
fn collection() -> FigValue {
    obj(vec![
        ("guid", guid(20)),
        ("type", FigValue::Str("VARIABLE_SET".into())),
        (
            "variableSetModes",
            FigValue::Array(
                vec![
                    obj(vec![
                        ("id", guid(31)),
                        ("sortPosition", FigValue::Str("&".into())),
                    ]),
                    obj(vec![
                        ("id", guid(30)),
                        ("sortPosition", FigValue::Str("%".into())),
                    ]),
                ]
                .into(),
            ),
        ),
    ])
}
fn modes(mode: u32) -> FigValue {
    obj(vec![(
        "entries",
        FigValue::Array(
            vec![obj(vec![
                ("variableSetID", obj(vec![("guid", guid(20))])),
                ("variableModeID", guid(mode)),
            ])]
            .into(),
        ),
    )])
}
fn paint(id: u32) -> FigValue {
    obj(vec![
        ("type", FigValue::Str("SOLID".into())),
        ("color", color(0.0, 1.0, 0.0, 1.0)),
        ("colorVar", alias(id)),
        ("opacity", FigValue::Float(0.8)),
    ])
}
fn layer(id: u32, bound: u32, parent: Option<u32>, mode: Option<u32>) -> FigValue {
    let mut node = obj(vec![
        ("guid", guid(id)),
        ("type", FigValue::Str("FRAME".into())),
        ("fillPaints", FigValue::Array(vec![paint(bound)].into())),
    ]);
    if let Some(parent) = parent {
        node.set("parentIndex", obj(vec![("guid", guid(parent))]));
    }
    if let Some(mode) = mode {
        node.set("variableModeBySetMap", modes(mode));
    }
    node
}
fn channel(node: &FigValue, channel: &str) -> f64 {
    node.get_array("fillPaints").unwrap()[0]
        .get("color")
        .unwrap()
        .get_f64(channel)
        .unwrap()
}
fn palette() -> Vec<FigValue> {
    vec![
        collection(),
        variable(10, 20, vec![(30, alias(11)), (31, alias(11))]),
        variable(11, 20, vec![(30, alias(12)), (31, alias(12))]),
        variable(
            12,
            20,
            vec![
                (30, literal(color(1.0, 0.0, 0.0, 0.25))),
                (31, literal(color(0.0, 0.0, 1.0, 1.0))),
            ],
        ),
    ]
}

#[test]
fn bound_alias_chain_replaces_stale_color_and_splits_solid_alpha_into_opacity() {
    let mut nodes = palette();
    nodes.push(layer(100, 10, None, None));
    resolve_color_variables(&mut nodes);
    let node = nodes.last().unwrap();
    assert_eq!(
        (
            channel(node, "r"),
            channel(node, "g"),
            channel(node, "b"),
            channel(node, "a")
        ),
        (1.0, 0.0, 0.0, 1.0)
    );
    let paint = &node.get_array("fillPaints").unwrap()[0];
    assert!(paint.get("colorVar").is_some());
    assert_eq!(paint.get_f64("opacity"), Some(0.25));
}

#[test]
fn bound_solid_alpha_reaches_the_fill_mapper_once_including_zero_and_one() {
    use crate::mappers::map_figma_fills;
    use jian_ops_schema::style::PenFill;

    for alpha in [0.08_f32, 0.6, 0.87, 0.38, 0.0, 1.0] {
        let mut node = layer(100, 10, None, None);
        node.set("opacity", FigValue::Float(0.5));
        let mut nodes = vec![
            variable(10, 20, vec![(30, literal(color(0.0, 0.0, 0.0, alpha)))]),
            node,
        ];
        resolve_color_variables(&mut nodes);
        let node = &nodes[1];
        let fills = map_figma_fills(node.get_array("fillPaints")).unwrap();
        let PenFill::Solid(fill) = &fills[0] else {
            panic!("expected the original solid paint");
        };
        assert_eq!(fill.color, "#000000");
        assert_eq!(fill.opacity, Some(alpha));
        assert_eq!(node.get_f64("opacity"), Some(0.5));
        if alpha == 0.08 {
            // The imported black 8% panel on white must be 235, rather than
            // the 253 produced by multiplying the same 8% alpha twice.
            assert_eq!((255.0 * (1.0 - fill.opacity.unwrap())).round(), 235.0);
        }
    }
}

#[test]
fn variable_mode_alpha_replaces_cached_solid_opacity() {
    let mut nodes = palette();
    nodes.extend([
        layer(100, 10, None, Some(30)),
        layer(101, 10, None, Some(31)),
    ]);
    resolve_color_variables(&mut nodes);
    assert_eq!(
        nodes[4].get_array("fillPaints").unwrap()[0].get_f64("opacity"),
        Some(0.25)
    );
    assert_eq!(
        nodes[5].get_array("fillPaints").unwrap()[0].get_f64("opacity"),
        Some(1.0)
    );
}

#[test]
fn gradient_stops_and_effects_keep_inline_variable_alpha() {
    let mut nodes = palette();
    let mut node = layer(100, 10, None, None);
    node.set(
        "fillPaints",
        FigValue::Array(
            vec![obj(vec![
                ("type", FigValue::Str("GRADIENT_LINEAR".into())),
                ("opacity", FigValue::Float(0.4)),
                (
                    "stops",
                    FigValue::Array(
                        vec![obj(vec![
                            ("position", FigValue::Float(0.0)),
                            ("color", color(0.0, 1.0, 0.0, 1.0)),
                            ("colorVar", alias(10)),
                        ])]
                        .into(),
                    ),
                ),
            ])]
            .into(),
        ),
    );
    node.set(
        "effects",
        FigValue::Array(
            vec![obj(vec![
                ("type", FigValue::Str("DROP_SHADOW".into())),
                ("color", color(0.0, 1.0, 0.0, 1.0)),
                ("colorVar", alias(10)),
            ])]
            .into(),
        ),
    );
    nodes.push(node);
    resolve_color_variables(&mut nodes);
    let node = &nodes[4];
    let gradient = &node.get_array("fillPaints").unwrap()[0];
    let stop = &gradient.get_array("stops").unwrap()[0];
    let effect = &node.get_array("effects").unwrap()[0];
    for value in [stop, effect] {
        assert_eq!(value.get("color").unwrap().get_f64("a"), Some(0.25));
        assert!(value.get("opacity").is_none());
    }
    assert!((gradient.get_f64("opacity").unwrap() - 0.4).abs() < 0.00001);
}

#[test]
fn unbound_and_unresolved_solid_paints_retain_the_original_encoding() {
    let mut nodes = palette();
    let unbound = obj(vec![
        ("type", FigValue::Str("SOLID".into())),
        ("color", color(0.1, 0.2, 0.3, 0.4)),
        ("opacity", FigValue::Float(0.6)),
    ]);
    let mut node = layer(100, 999, None, None);
    node.set(
        "fillPaints",
        FigValue::Array(vec![unbound, paint(999)].into()),
    );
    let original = node.clone();
    nodes.push(node);
    resolve_color_variables(&mut nodes);
    assert_eq!(nodes[4], original);
}

#[test]
fn node_mode_overrides_parent_mode_and_aliases_use_inherited_collection_mode() {
    let mut nodes = palette();
    nodes.extend([
        layer(100, 10, None, Some(31)),
        layer(101, 10, Some(100), None),
        layer(102, 10, Some(100), Some(30)),
    ]);
    resolve_color_variables(&mut nodes);
    assert_eq!(channel(&nodes[4], "b"), 1.0);
    assert_eq!(channel(&nodes[5], "b"), 1.0);
    assert_eq!(channel(&nodes[6], "r"), 1.0);
}

#[test]
fn missing_variable_unknown_mode_and_alias_cycle_keep_cached_fallback() {
    let mut nodes = palette();
    nodes.extend([
        variable(13, 20, vec![(30, alias(14))]),
        variable(14, 20, vec![(30, alias(13))]),
        layer(100, 999, None, None),
        layer(101, 10, None, Some(99)),
        layer(102, 13, None, None),
    ]);
    resolve_color_variables(&mut nodes);
    for node in &nodes[6..] {
        assert_eq!(channel(node, "g"), 1.0);
        assert_eq!(channel(node, "r"), 0.0);
    }
}

#[test]
fn nested_instance_override_and_gradient_stop_bindings_resolve_in_node_mode() {
    let mut nodes = palette();
    let mut node = layer(100, 10, None, Some(31));
    node.set(
        "symbolData",
        obj(vec![(
            "symbolOverrides",
            FigValue::Array(
                vec![obj(vec![
                    ("strokePaints", FigValue::Array(vec![paint(10)].into())),
                    (
                        "fillPaints",
                        FigValue::Array(
                            vec![obj(vec![(
                                "stops",
                                FigValue::Array(vec![paint(10)].into()),
                            )])]
                            .into(),
                        ),
                    ),
                ])]
                .into(),
            ),
        )]),
    );
    nodes.push(node);
    resolve_color_variables(&mut nodes);
    let entry = &nodes[4]
        .get("symbolData")
        .unwrap()
        .get_array("symbolOverrides")
        .unwrap()[0];
    let stroke = &entry.get_array("strokePaints").unwrap()[0];
    let stop = &entry.get_array("fillPaints").unwrap()[0]
        .get_array("stops")
        .unwrap()[0];
    for value in [stroke, stop] {
        assert_eq!(value.get("color").unwrap().get_f64("b"), Some(1.0));
    }
}

#[test]
fn shared_master_cached_paints_survive_different_instance_placement_modes() {
    let mut nodes = palette();
    let mut master = layer(200, 10, None, Some(30));
    master.set("type", FigValue::Str("SYMBOL".into()));
    let mut instance = layer(202, 10, None, Some(31));
    instance.set("type", FigValue::Str("INSTANCE".into()));
    instance.set(
        "symbolData",
        obj(vec![
            ("symbolID", guid(200)),
            (
                "symbolOverrides",
                FigValue::Array(
                    vec![obj(vec![(
                        "fillPaints",
                        FigValue::Array(vec![paint(10)].into()),
                    )])]
                    .into(),
                ),
            ),
        ]),
    );
    nodes.extend([master, layer(201, 10, Some(200), None), instance]);
    resolve_color_variables(&mut nodes);
    // The pre-expansion pass cannot choose a color for both placements of a
    // shared master. Preserve its original cache rather than bake one theme.
    assert_eq!(channel(&nodes[4], "g"), 1.0);
    assert_eq!(channel(&nodes[5], "g"), 1.0);
    assert_eq!(channel(&nodes[6], "b"), 1.0);
    let override_paint = &nodes[6]
        .get("symbolData")
        .unwrap()
        .get_array("symbolOverrides")
        .unwrap()[0]
        .get_array("fillPaints")
        .unwrap()[0];
    assert_eq!(override_paint.get("color").unwrap().get_f64("b"), Some(1.0));
}
