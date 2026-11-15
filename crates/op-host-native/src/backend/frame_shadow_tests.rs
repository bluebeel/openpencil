use super::{NativeBackend, NativeFrameBackend};
use op_editor_ui::{Point2D, RenderBackend};

#[test]
fn shadow_offsets_keep_the_frame_basis_through_node_affine_transforms() {
    let mut surface = skia_safe::surfaces::raster_n32_premul((32, 32)).unwrap();
    let canvas = surface.canvas();
    canvas.translate((9.0, -7.0));
    canvas.scale((2.0, 3.0));
    let basis = canvas.local_to_device_as_3x3();
    let mut backend = NativeBackend::with_dpi(1.0);
    let mut frame = NativeFrameBackend::new(&mut backend, canvas);
    let offset = Point2D::new(7.0, -11.0);
    assert_eq!(frame.shadow_offset_in_local_space(offset), offset);
    for (degrees, scale) in [
        (30.0_f32, Point2D::new(1.0, 1.0)),
        (-30.0, Point2D::new(-1.0, 1.0)),
        (90.0, Point2D::new(1.5, -0.5)),
        (-90.0, Point2D::new(-0.75, -2.0)),
    ] {
        frame.save();
        frame.scale(scale, Point2D::new(4.0, 8.0));
        frame.rotate(degrees.to_radians(), Point2D::new(13.0, 6.0));
        let local = frame.shadow_offset_in_local_space(offset);
        let actual = canvas
            .local_to_device_as_3x3()
            .map_vector((local.x, local.y));
        let expected = basis.map_vector((offset.x, offset.y));
        assert!((actual.x - expected.x).abs() < 0.00002);
        assert!((actual.y - expected.y).abs() < 0.00002);
        frame.restore();
        assert_eq!(frame.shadow_offset_in_local_space(offset), offset);
    }
    frame.scale(Point2D::new(0.0, 1.0), Point2D::new(0.0, 0.0));
    assert_eq!(frame.shadow_offset_in_local_space(offset), offset);
}
