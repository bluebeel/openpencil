//! Native raster regressions through the shared desktop/export painter.

use super::paint_node;
use op_editor_ui::layout_scene::{DropShadow, Effect, NodeKind, SceneNode};
use op_editor_ui::{Color, Rect};
use skia_safe::{AlphaType, ColorType, ImageInfo, Surface};

const SIZE: i32 = 128;

fn card(spread: f32, blur: f32) -> SceneNode {
    let mut node = SceneNode::leaf("card", NodeKind::Rect);
    node.bounds = Rect::xywh(40.0, 40.0, 60.0, 60.0);
    node.corner_radius = 12.0;
    node.fill = Some(Color::WHITE);
    node.effects = vec![Effect::DropShadow(DropShadow {
        offset_x: 12.0,
        offset_y: 0.0,
        blur,
        spread,
        color: Color::BLACK,
        inner: false,
    })];
    node
}

fn surface() -> Surface {
    let mut surface = skia_safe::surfaces::raster_n32_premul((SIZE, SIZE)).unwrap();
    surface.canvas().clear(skia_safe::Color::WHITE);
    surface
}

fn rgba(surface: &mut Surface) -> Vec<u8> {
    let info = ImageInfo::new((SIZE, SIZE), ColorType::RGBA8888, AlphaType::Unpremul, None);
    let mut pixels = vec![0; (SIZE * SIZE * 4) as usize];
    assert!(surface.image_snapshot().read_pixels(
        &info,
        &mut pixels,
        (SIZE * 4) as usize,
        (0, 0),
        skia_safe::image::CachingHint::Allow
    ));
    pixels
}

fn rendered(spread: f32, blur: f32) -> Vec<u8> {
    let mut surface = surface();
    paint_node(surface.canvas(), &card(spread, blur));
    rgba(&mut surface)
}

fn red(pixels: &[u8], x: usize, y: usize) -> u8 {
    pixels[(y * SIZE as usize + x) * 4]
}

#[test]
fn native_raster_honors_positive_negative_and_collapsed_spread() {
    let contracted = rendered(-5.0, 0.0);
    let original = rendered(0.0, 0.0);
    let expanded = rendered(5.0, 0.0);
    let collapsed = rendered(-30.0, 0.0);
    assert_eq!(red(&contracted, 109, 70), 255);
    assert_eq!(red(&original, 109, 70), 0);
    assert_eq!(red(&original, 115, 70), 255);
    assert_eq!(red(&expanded, 115, 70), 0);
    assert_eq!(red(&collapsed, 103, 70), 255);
}

#[test]
fn native_negative_five_spread_contracts_the_blurred_shadow_halo() {
    let contracted = rendered(-5.0, 10.0);
    let original = rendered(0.0, 10.0);
    let expanded = rendered(5.0, 10.0);
    let halo_ink = |pixels: &[u8]| -> u32 {
        (101..125)
            .flat_map(|x| (50..90).map(move |y| u32::from(255 - red(pixels, x, y))))
            .sum()
    };
    assert!(halo_ink(&contracted) < halo_ink(&original));
    assert!(halo_ink(&original) < halo_ink(&expanded));
}

#[test]
fn zero_spread_native_raster_matches_the_previous_backend_primitive() {
    let actual = rendered(0.0, 10.0);
    let mut expected = surface();
    let backend = op_host_native::NativeBackend::with_dpi(1.0);
    backend.fill_drop_shadow(
        expected.canvas(),
        Rect::xywh(52.0, 40.0, 60.0, 60.0),
        12.0,
        10.0,
        Color::BLACK,
    );
    backend.fill_round_rect(
        expected.canvas(),
        Rect::xywh(40.0, 40.0, 60.0, 60.0),
        12.0,
        Color::WHITE,
    );
    assert_eq!(actual, rgba(&mut expected));
}

#[test]
fn rotated_and_reflected_outer_shadows_keep_canvas_axis_offsets() {
    let backend = op_host_native::NativeBackend::with_dpi(1.0);
    for degrees in [0.0_f32, -90.0, 90.0, 180.0] {
        for flip_x in [false, true] {
            let mut node = card(0.0, 0.0);
            node.bounds = Rect::xywh(44.0, 48.0, 36.0, 24.0);
            node.corner_radius = 0.0;
            node.fill = None;
            node.rotation = degrees.to_radians();
            node.flip_x = flip_x;
            let mut actual = surface();
            paint_node(actual.canvas(), &node);
            let mut expected = surface();
            let canvas = expected.canvas();
            // Apply displacement before the shape transform: this independent
            // reference translates the transformed silhouette in canvas axes.
            canvas.translate((12.0, 0.0));
            let pivot = skia_safe::Point::new(62.0, 60.0);
            if flip_x {
                canvas.translate(pivot);
                canvas.scale((-1.0, 1.0));
                canvas.translate((-pivot.x, -pivot.y));
            }
            canvas.rotate(degrees, Some(pivot));
            backend.fill_drop_shadow(canvas, node.bounds, 0.0, 0.0, Color::BLACK);
            assert_eq!(
                rgba(&mut actual),
                rgba(&mut expected),
                "{degrees}, {flip_x}"
            );
        }
    }
}

#[test]
fn rotated_svg_inner_shadows_keep_canvas_axis_offsets() {
    let mut backend = op_host_native::NativeBackend::with_dpi(1.0);
    let d = "M0 0H36V24H0Z";
    for degrees in [-90.0_f32, 0.0, 90.0] {
        let mut node = SceneNode::leaf("inset", NodeKind::Path);
        node.bounds = Rect::xywh(44.0, 48.0, 36.0, 24.0);
        node.svg_path = Some(d.into());
        node.rotation = degrees.to_radians();
        node.fill = None;
        node.effects = vec![Effect::DropShadow(DropShadow {
            offset_x: 8.0,
            offset_y: 4.0,
            blur: 0.0,
            spread: 0.0,
            color: Color::BLACK,
            inner: true,
        })];
        let mut actual = surface();
        paint_node(actual.canvas(), &node);
        let mut expected = surface();
        let canvas = expected.canvas();
        canvas.rotate(degrees, Some(skia_safe::Point::new(62.0, 60.0)));
        let (sin, cos) = degrees.to_radians().sin_cos();
        backend.fill_inner_shadow_svg_path(
            canvas,
            d,
            node.bounds,
            8.0 * cos + 4.0 * sin,
            -8.0 * sin + 4.0 * cos,
            0.0,
            Color::BLACK,
        );
        assert_eq!(rgba(&mut actual), rgba(&mut expected), "{degrees}");
    }
}
