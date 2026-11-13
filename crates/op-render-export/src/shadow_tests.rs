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
