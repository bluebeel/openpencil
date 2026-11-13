//! Authored auto-layout stacking must survive Preserve-mode imports.

use super::*;

#[test]
fn reverse_stack_changes_preserved_paint_order_without_reversing_layout_flow() {
    for component in [false, true] {
        for layout_mode in [FigLayoutMode::Preserve, FigLayoutMode::OpenPencil] {
            for reverse_z_index in [false, true] {
                let mut tree = auto_layout_frame(vec![
                    rect_child("background", 2, "#"),
                    rect_child("foreground", 1, "!"),
                ]);
                add_fields(
                    &mut tree,
                    vec![("stackReverseZIndex", FigValue::Bool(reverse_z_index))],
                );
                for child in &mut tree.children {
                    add_fields(child, vec![("transform", transform(8.0, 12.0))]);
                }
                let mut ctx = fresh_ctx();
                ctx.layout_mode = layout_mode;
                let node = if component {
                    convert_component(&tree, None, &mut ctx)
                } else {
                    convert_frame(&tree, None, &mut ctx)
                };
                let expected = if layout_mode == FigLayoutMode::OpenPencil || reverse_z_index {
                    ["foreground", "background"]
                } else {
                    ["background", "foreground"]
                };
                assert_eq!(child_names(&node), expected);
                let PenNode::Frame(frame) = node else {
                    panic!("expected frame");
                };
                for child in frame.children.unwrap() {
                    let PenNode::Rectangle(rect) = child else {
                        panic!("expected rectangle");
                    };
                    let coordinates = if layout_mode == FigLayoutMode::Preserve {
                        (Some(8.0), Some(12.0))
                    } else {
                        (None, None)
                    };
                    assert_eq!((rect.base.x, rect.base.y), coordinates);
                }
            }
        }
    }
}

#[test]
fn preserved_instances_inherit_stack_order_unless_the_instance_overrides_it() {
    let mut symbol = auto_layout_frame(vec![
        rect_child("background", 2, "#"),
        rect_child("foreground", 1, "!"),
    ]);
    add_fields(
        &mut symbol,
        vec![("stackReverseZIndex", FigValue::Bool(true))],
    );
    for override_value in [None, Some(false), Some(true)] {
        let mut instance = TreeNode {
            figma: obj(vec![
                ("type", FigValue::Str("INSTANCE".into())),
                (
                    "symbolData",
                    obj(vec![(
                        "symbolID",
                        obj(vec![
                            ("sessionID", FigValue::Uint(1)),
                            ("localID", FigValue::Uint(100)),
                        ]),
                    )]),
                ),
            ]),
            children: Vec::new(),
        };
        if let Some(value) = override_value {
            add_fields(
                &mut instance,
                vec![("stackReverseZIndex", FigValue::Bool(value))],
            );
        }
        let mut ctx = fresh_ctx();
        ctx.layout_mode = FigLayoutMode::Preserve;
        ctx.symbol_tree.insert("1:100".into(), &symbol);
        let node = convert_instance(&instance, None, &mut ctx);
        let expected = if override_value == Some(false) {
            ["background", "foreground"]
        } else {
            ["foreground", "background"]
        };
        assert_eq!(child_names(&node), expected);
    }
}
