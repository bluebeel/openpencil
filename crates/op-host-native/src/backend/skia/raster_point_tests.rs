use super::super::NativeBackend;
use super::*;
use op_editor_ui::{Point2D, TextLayout};

fn foreground() -> Color {
    Color {
        r: 190.0 / 255.0,
        g: 41.0 / 255.0,
        b: 77.0 / 255.0,
        a: 0.35,
    }
}
fn surface() -> skia_safe::Surface {
    let mut surface = skia_safe::surfaces::raster_n32_premul((64, 48)).unwrap();
    surface
        .canvas()
        .clear(skia_safe::Color::from_rgb(11, 37, 83));
    surface
}
fn pixel(surface: &mut skia_safe::Surface, p: (i32, i32)) -> skia_safe::Color {
    surface.image_snapshot().peek_pixels().unwrap().get_color(p)
}

#[test]
fn correlated_rect_stroke_and_mesh_keep_float_alpha_before_framebuffer_write() {
    let backend = NativeBackend::with_dpi(1.0);
    jian_skia::raster_sampling::with_point_sampling(|| {
        let mut fill = surface();
        assert!(rect(
            fill.canvas(),
            Rect::xywh(1.0, 1.0, 6.0, 6.0),
            foreground(),
            None
        ));
        assert_eq!(
            pixel(&mut fill, (3, 3)),
            skia_safe::Color::from_rgb(74, 38, 81)
        );
        let mut stroke = surface();
        assert!(rect(
            stroke.canvas(),
            Rect::xywh(1.0, 1.0, 6.0, 6.0),
            foreground(),
            Some(2.0)
        ));
        assert_eq!(
            pixel(&mut stroke, (1, 3)),
            skia_safe::Color::from_rgb(74, 38, 81)
        );
        let mut gradient = surface();
        backend.fill_round_rect_mesh_gradient(
            gradient.canvas(),
            Rect::xywh(0.0, 0.0, 8.0, 8.0),
            0.0,
            2,
            2,
            &[foreground(); 4],
            1.0,
        );
        assert_eq!(
            pixel(&mut gradient, (3, 3)),
            skia_safe::Color::from_rgb(74, 38, 81)
        );
        let mut translucent = surface();
        backend.fill_round_rect_mesh_gradient(
            translucent.canvas(),
            Rect::xywh(0.0, 0.0, 8.0, 8.0),
            0.0,
            2,
            2,
            &[foreground(); 4],
            0.5,
        );
        assert_eq!(
            pixel(&mut translucent, (3, 3)),
            skia_safe::Color::from_rgb(42, 38, 82),
            "mesh opacity applies once after float interpolation"
        );
    })
    .unwrap();
}

#[test]
fn correlated_text_keeps_original_float_color_for_direct_and_shaped_runs() {
    jian_skia::with_font_lock(|| {
        jian_skia::register_imported_font(
            include_bytes!("../../../../op-host-desktop/assets/fonts/Inter-VF.ttf").to_vec(),
        )
        .unwrap();
        let mut backend = NativeBackend::with_dpi(1.0);
        for kerning in [None, Some(true)] {
            let layout =
                TextLayout::single_run("M", "Inter", 36.0, foreground().to_jian(), Point2D::ZERO)
                    .with_source_color(foreground())
                    .with_kerning(kerning);
            let mut point = surface();
            jian_skia::raster_sampling::with_point_sampling(|| {
                backend.draw_text(point.canvas(), &layout, Point2D::new(2.0, 40.0))
            })
            .unwrap();
            let image = point.image_snapshot();
            let pixels = image.peek_pixels().unwrap();
            let maximum = (0..48)
                .flat_map(|y| (0..64).map(move |x| (x, y)))
                .map(|p| pixels.get_color(p).r())
                .max()
                .unwrap();
            assert_eq!(
                maximum, 74,
                "float source color survives kerning={kerning:?}"
            );
            let mut legacy = surface();
            backend.draw_text(legacy.canvas(), &layout, Point2D::new(2.0, 40.0));
            let plain =
                TextLayout::single_run("M", "Inter", 36.0, foreground().to_jian(), Point2D::ZERO)
                    .with_kerning(kerning);
            let mut control = surface();
            backend.draw_text(control.canvas(), &plain, Point2D::new(2.0, 40.0));
            assert_eq!(
                legacy.image_snapshot().peek_pixels().unwrap().bytes(),
                control.image_snapshot().peek_pixels().unwrap().bytes(),
                "Native default ignores optional source metadata"
            );
        }
    });
}

fn shadow_sigma(scale: i32, point: bool) -> f64 {
    let backend = NativeBackend::with_dpi(1.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((192 * scale, 128 * scale)).unwrap();
    surface.canvas().scale((scale as f32, scale as f32));
    let mut draw = || {
        backend.fill_drop_shadow(
            surface.canvas(),
            Rect::xywh(64.0, 32.0, 64.0, 64.0),
            0.0,
            12.0,
            Color::WHITE,
        )
    };
    if point {
        jian_skia::raster_sampling::with_point_sampling(draw).unwrap();
    } else {
        draw();
    }
    let image = surface.image_snapshot();
    let pixels = image.peek_pixels().unwrap();
    let mut mass = 0f64;
    let mut mean = 0f64;
    for x in 0..192 * scale {
        let a = f64::from(pixels.get_color((x, 64 * scale)).a());
        mass += a;
        mean += a * (f64::from(x) + 0.5) / f64::from(scale);
    }
    mean /= mass;
    let mut variance = 0f64;
    for x in 0..192 * scale {
        let a = f64::from(pixels.get_color((x, 64 * scale)).a());
        variance += a * ((f64::from(x) + 0.5) / f64::from(scale) - mean).powi(2);
    }
    (variance / mass - 64.0 * 64.0 / 12.0).max(0.0).sqrt()
}

#[test]
fn correlated_shadow_width_survives_camera_resolution_and_live_device_behavior_is_unchanged() {
    // The second moment of a Gaussian-convolved box is box variance+sigma².
    // Estimate in logical coordinates, without a fitted output-pixel offset.
    let native = shadow_sigma(1, false);
    for scale in [2, 3] {
        let camera = shadow_sigma(scale, true);
        assert!(
            (camera - native).abs() < 0.25,
            "camera{scale} changed logical shadow sigma {native}->{camera}"
        );
    }
    assert!(
        shadow_sigma(2, false) < native * 0.6,
        "legacy device-fixed Native sigma is preserved"
    );
}
