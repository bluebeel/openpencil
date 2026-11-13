//! Derived instance geometry must not inherit a second parent resize.

use super::tests::{guid, leaf, obj, ov_with, size, symbol_root};
use super::*;

#[test]
fn geometry_derived_endpoint_stays_round_while_unresolved_siblings_scale() {
    for geometry_key in ["fillGeometry", "strokeGeometry"] {
        let geometry = FigValue::Array(vec![obj(vec![("commandsBlob", FigValue::Uint(1))])]);
        let mut children = Vec::new();
        for id in [11, 12, 13] {
            let mut child = leaf("endpoint", 1, id);
            child.figma.set("type", FigValue::Str("ELLIPSE".into()));
            child.figma.set("size", size(8.0, 8.0));
            // An authored master cache alone does not establish instance space.
            child.figma.set(geometry_key, geometry.clone());
            children.push(child);
        }
        let mut pointer = leaf("pointer", 1, 10);
        pointer.figma.set("type", FigValue::Str("FRAME".into()));
        pointer.figma.set("size", size(100.0, 8.0));
        pointer.children = children;
        let mut symbol = symbol_root(vec![pointer]);
        symbol.figma.set("size", size(100.0, 8.0));
        let derived = vec![
            ov_with(vec![guid(1, 10)], vec![("size", size(200.0, 8.0))]),
            ov_with(vec![guid(1, 11)], vec![(geometry_key, geometry)]),
            ov_with(
                vec![guid(1, 12)],
                vec![(geometry_key, FigValue::Array(Vec::new()))],
            ),
        ];
        let converted = apply_instance_overrides(
            &symbol,
            None,
            Some(&derived),
            Some(FigVec2 { x: 200.0, y: 8.0 }),
        );
        let endpoints = &converted[0].children;
        let dimensions: Vec<_> = endpoints
            .iter()
            .map(|node| FigVec2::from_value(node.figma.get("size").unwrap()).unwrap())
            .collect();
        assert_eq!((dimensions[0].x, dimensions[0].y), (8.0, 8.0));
        assert_eq!(
            (dimensions[1].x, dimensions[1].y),
            (16.0, 8.0),
            "an empty derived geometry array must not suppress scaling"
        );
        assert_eq!(
            (dimensions[2].x, dimensions[2].y),
            (16.0, 8.0),
            "an authored master cache without derived evidence must still scale"
        );
    }
}

#[test]
fn auto_layout_resize_preserves_borders_without_changing_uniform_scale_or_overrides() {
    let cases = [
        ("width resize", "HORIZONTAL", 50.0, 40.0, None, 2.0),
        ("height resize", "VERTICAL", 100.0, 20.0, None, 2.0),
        ("uniform shrink", "HORIZONTAL", 50.0, 20.0, None, 1.0),
        ("explicit stroke", "HORIZONTAL", 50.0, 20.0, Some(3.0), 3.0),
        ("non-layout artwork", "NONE", 50.0, 40.0, None, 1.0),
    ];
    for (name, stack_mode, width, height, stroke_override, expected) in cases {
        let mut frame = leaf(name, 1, 10);
        frame.figma.set("type", FigValue::Str("FRAME".into()));
        frame
            .figma
            .set("stackMode", FigValue::Str(stack_mode.into()));
        frame.figma.set("size", size(100.0, 40.0));
        frame.figma.set("strokeWeight", FigValue::Float(2.0));
        let mut symbol = symbol_root(vec![frame]);
        symbol.figma.set("size", size(100.0, 40.0));
        let derived = vec![ov_with(
            vec![guid(1, 10)],
            vec![("size", size(width, height))],
        )];
        let overrides = stroke_override.map(|weight| {
            vec![ov_with(
                vec![guid(1, 10)],
                vec![("strokeWeight", FigValue::Float(weight))],
            )]
        });
        let converted =
            apply_instance_overrides(&symbol, overrides.as_deref(), Some(&derived), None);
        assert_eq!(
            converted[0].figma.get_f64("strokeWeight"),
            Some(expected),
            "{name}"
        );
    }
}
