use super::test_support::{filled_rect, scene_with};
use super::*;
use op_editor_ui::layout_scene::NodeKind;
use op_editor_ui::Color;

#[test]
fn explicit_viewport_keeps_transparent_frame_padding() {
    let mut frame = SceneNode::leaf("frame", NodeKind::Group);
    frame.bounds = Rect::xywh(100.0, 200.0, 20.0, 10.0);
    frame
        .children
        .push(filled_rect("child", 104.0, 203.0, 6.0, 2.0, Color::BLACK));
    let bounds = frame.bounds;
    let scene = scene_with(vec![frame]);
    let path = std::env::temp_dir().join(format!("op-frame-viewport-{}.png", std::process::id()));
    export_node_raster_with_viewport(&scene, "frame", &path, RasterFormat::Png, 1.0, bounds)
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(path);
    assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 20);
    assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 10);
    let image = skia_safe::Image::from_encoded(skia_safe::Data::new_copy(&bytes)).unwrap();
    let info = skia_safe::ImageInfo::new(
        (20, 10),
        skia_safe::ColorType::RGBA8888,
        skia_safe::AlphaType::Unpremul,
        None,
    );
    let mut pixels = vec![0u8; 20 * 10 * 4];
    assert!(image.read_pixels(
        &info,
        &mut pixels,
        20 * 4,
        (0, 0),
        skia_safe::image::CachingHint::Allow
    ));
    assert_eq!(&pixels[..4], &[0, 0, 0, 0]);
    assert_eq!(
        &pixels[(3 * 20 + 4) * 4..(3 * 20 + 4) * 4 + 4],
        &[0, 0, 0, 255]
    );
}
