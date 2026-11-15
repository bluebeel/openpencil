use super::*;
use crate::layout_scene::{BlurEffect, DropShadow};
use crate::Color;

fn rect(id: &str, bounds: Rect) -> SceneNode {
    let mut node = SceneNode::leaf(id, NodeKind::Rect);
    node.bounds = bounds;
    node.fill = Some(Color::RED);
    node
}

#[test]
fn node_bounds_union_descendants_then_transform_to_viewport() {
    let mut root = rect("root", Rect::xywh(100.0, 200.0, 20.0, 10.0));
    root.kind = NodeKind::Frame;
    root.children = vec![rect("child", Rect::xywh(140.0, 190.0, 5.0, 5.0))];
    assert_eq!(
        node_composite_layer_bounds(
            &root,
            Point2D::new(10.0, 20.0),
            2.0,
            Rect::xywh(0.0, 0.0, 1_000.0, 1_000.0),
            None,
            &[],
        ),
        Rect::xywh(209.0, 399.0, 92.0, 42.0)
    );
}

#[test]
fn shadow_and_layer_blur_expand_completed_subtree() {
    let mut node = rect("effects", Rect::xywh(10.0, 10.0, 20.0, 20.0));
    node.effects = vec![
        Effect::DropShadow(DropShadow {
            blur_kernel: Default::default(),
            offset_x: 10.0,
            offset_y: -5.0,
            blur: 8.0,
            spread: 0.0,
            color: Color::BLACK,
            inner: false,
        }),
        Effect::Blur(BlurEffect { radius: 4.0 }),
    ];
    assert_eq!(
        node_composite_layer_bounds(
            &node,
            Point2D::ZERO,
            1.0,
            Rect::xywh(-100.0, -100.0, 1_000.0, 1_000.0),
            None,
            &[],
        ),
        Rect::xywh(1.0, -14.0, 58.0, 58.0)
    );
}

fn horizontally_flipped_shadow_node(id: &str) -> SceneNode {
    let mut node = rect(id, Rect::xywh(0.0, 0.0, 10.0, 10.0));
    node.flip_x = true;
    node.effects = vec![Effect::DropShadow(DropShadow {
        blur_kernel: Default::default(),
        offset_x: 20.0,
        offset_y: 0.0,
        blur: 0.0,
        spread: 0.0,
        color: Color::BLACK,
        inner: false,
    })];
    node
}

#[test]
fn signed_spread_is_included_in_effect_bounds_and_collapse_removes_the_shadow() {
    for (spread, expected) in [
        (5.0, Rect::xywh(9.0, 4.0, 47.0, 32.0)),
        (-5.0, Rect::xywh(9.0, 9.0, 37.0, 22.0)),
        (-10.0, Rect::xywh(9.0, 9.0, 22.0, 22.0)),
    ] {
        let mut node = rect("spread", Rect::xywh(10.0, 10.0, 20.0, 20.0));
        node.effects = vec![Effect::DropShadow(DropShadow {
            blur_kernel: Default::default(),
            offset_x: 20.0,
            offset_y: 0.0,
            blur: 0.0,
            spread,
            color: Color::BLACK,
            inner: false,
        })];
        assert_eq!(
            node_composite_layer_bounds(
                &node,
                Point2D::ZERO,
                1.0,
                Rect::xywh(-100.0, -100.0, 1000.0, 1000.0),
                None,
                &[]
            ),
            expected
        );
    }
}

#[test]
fn descendant_flip_keeps_canvas_axis_shadow_before_parent_layer_bounds() {
    let mut root = SceneNode::leaf("root", NodeKind::Group);
    root.children = vec![horizontally_flipped_shadow_node("child")];
    assert_eq!(
        node_composite_layer_bounds(
            &root,
            Point2D::ZERO,
            1.0,
            Rect::xywh(-100.0, -100.0, 500.0, 500.0),
            None,
            &[],
        ),
        Rect::xywh(-1.0, -1.0, 32.0, 12.0)
    );
}

#[test]
fn mask_root_flip_keeps_canvas_axis_shadow_in_mask_layer_bounds() {
    let mask = horizontally_flipped_shadow_node("mask");
    assert_eq!(
        sibling_mask_layer_bounds(
            &mask,
            std::iter::empty(),
            Point2D::ZERO,
            1.0,
            Rect::xywh(-100.0, -100.0, 500.0, 500.0),
            None,
            &[],
        ),
        Rect::xywh(-1.0, -1.0, 32.0, 12.0)
    );
}

#[test]
fn mask_bounds_union_only_supplied_run_and_intersect_cull() {
    let mask = rect("mask", Rect::xywh(0.0, 0.0, 10.0, 10.0));
    let content = rect("content", Rect::xywh(20.0, 0.0, 10.0, 10.0));
    assert_eq!(
        sibling_mask_layer_bounds(
            &mask,
            [&content],
            Point2D::ZERO,
            1.0,
            Rect::xywh(0.0, 0.0, 25.0, 100.0),
            None,
            &[],
        ),
        Rect::xywh(0.0, 0.0, 25.0, 11.0)
    );
}

#[test]
fn rotated_ancestor_cull_is_inverse_mapped_before_intersection() {
    let node = rect("nested", Rect::xywh(10.0, 0.0, 10.0, 10.0));
    let ancestor = OverlayTransform {
        rotation: std::f32::consts::FRAC_PI_2,
        flip_x: false,
        flip_y: false,
        pivot: Point2D::ZERO,
    };
    // The node's padded local bounds are (9,-1)..(21,11). After a
    // quarter turn they occupy (-11,9)..(1,21). This visible global
    // slice maps back to local (10,0)..(20,5), rather than appearing
    // disjoint when compared without undoing the ancestor rotation.
    let actual = node_composite_layer_bounds(
        &node,
        Point2D::ZERO,
        1.0,
        Rect::xywh(-5.0, 10.0, 5.0, 10.0),
        None,
        &[ancestor],
    );
    let expected = Rect::xywh(10.0, 0.0, 10.0, 5.0);
    for (actual, expected) in [
        (actual.origin.x, expected.origin.x),
        (actual.origin.y, expected.origin.y),
        (actual.size.x, expected.size.x),
        (actual.size.y, expected.size.y),
    ] {
        assert!((actual - expected).abs() < 0.0001, "{actual} != {expected}");
    }
}

#[test]
fn invalid_imported_geometry_falls_back_to_finite_cull() {
    let mut node = rect("bad", Rect::xywh(f32::NAN, 0.0, 10.0, 10.0));
    node.effects = vec![Effect::Blur(BlurEffect {
        radius: f32::INFINITY,
    })];
    let cull = Rect::xywh(5.0, 6.0, 700.0, 500.0);
    assert_eq!(
        node_composite_layer_bounds(&node, Point2D::ZERO, 1.0, cull, None, &[]),
        cull
    );
}
#[test]
fn combined_flip_rotation_bounds_match_canvas_concatenation_order() {
    let rect = Rect::xywh(1.0, 2.0, 3.0, 5.0);
    let rotation = std::f32::consts::FRAC_PI_2;
    let actual = transformed_aabb(rect, true, false, rotation, Point2D::ZERO).unwrap();
    for (a, b) in [
        actual.origin.x,
        actual.origin.y,
        actual.size.x,
        actual.size.y,
    ]
    .into_iter()
    .zip([2.0, 1.0, 5.0, 3.0])
    {
        assert!((a - b).abs() < 0.00001);
    }
    let inverse = inverse_transform_rect(
        actual,
        &[OverlayTransform {
            rotation,
            flip_x: true,
            flip_y: false,
            pivot: Point2D::ZERO,
        }],
    )
    .unwrap();
    for (a, b) in [
        inverse.origin.x,
        inverse.origin.y,
        inverse.size.x,
        inverse.size.y,
    ]
    .into_iter()
    .zip([1.0, 2.0, 3.0, 5.0])
    {
        assert!((a - b).abs() < 0.00001);
    }
}

#[test]
fn ancestor_rotation_and_flip_keep_descendant_shadow_on_canvas_x_axis() {
    let mut root = SceneNode::leaf("root", NodeKind::Group);
    root.rotation = std::f32::consts::FRAC_PI_2;
    root.flip_x = true;
    root.children = vec![horizontally_flipped_shadow_node("child")];
    let bounds = sibling_mask_layer_bounds(
        &root,
        std::iter::empty(),
        Point2D::ZERO,
        1.0,
        Rect::xywh(-100.0, -100.0, 500.0, 500.0),
        None,
        &[],
    );
    for (a, b) in [
        bounds.origin.x,
        bounds.origin.y,
        bounds.size.x,
        bounds.size.y,
    ]
    .into_iter()
    .zip([-1.0, -1.0, 32.0, 12.0])
    {
        assert!((a - b).abs() < 0.00001);
    }
}
