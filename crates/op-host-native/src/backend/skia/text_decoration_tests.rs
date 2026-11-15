use super::*;
use op_editor_ui::{Color, Point2D};
use std::collections::BTreeMap;

fn request<'a>(text: &'a str, features: &'a BTreeMap<String, u32>) -> TextDecorationRequest<'a> {
    TextDecorationRequest {
        text,
        font_family: "Roboto",
        font_size: 16.0,
        font_weight: 400,
        italic: false,
        kerning: Some(false),
        letter_spacing: 0.0,
        line_height: 1.5,
        justify_extra: 0.0,
        font_features: features,
        origin: Point2D::new(4.25, 65.46875),
        width: 120.0,
        color: Color {
            r: 0.1,
            g: 0.3,
            b: 0.7,
            a: 0.6,
        },
        underline: true,
        strikethrough: false,
        skip_ink: false,
    }
}

#[test]
fn decoration_font_metrics_and_local_baseline_match_independent_sizes() {
    jian_skia::with_font_lock(|| {
        let mut backend = NativeBackend::with_dpi(1.0);
        let features = BTreeMap::new();
        for (size, baseline, top, thickness) in [
            (16.0, 65.46875, 66.5625, 0.78125),
            (15.0, 18.126953, 19.464844, 0.7324219),
        ] {
            let mut r = request("Typography", &features);
            r.font_size = size;
            r.origin.y = baseline;
            let actual = geometry(&mut backend, &r).unwrap();
            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0], Rect::xywh(r.origin.x, top, r.width, thickness));
        }
    });
}

#[test]
fn decoration_skip_ink_preserves_explicit_false_and_merges_descender_intervals() {
    jian_skia::with_font_lock(|| {
        let mut backend = NativeBackend::with_dpi(1.0);
        let features = BTreeMap::new();
        let mut r = request("pyp", &features);
        let solid = geometry(&mut backend, &r).unwrap();
        assert_eq!(solid.len(), 1);
        r.skip_ink = true;
        let skipped = geometry(&mut backend, &r).unwrap();
        assert!(skipped.len() > 1);
        assert!(skipped.iter().map(|r| r.size.x).sum::<f32>() < r.width);
        for rect in &skipped {
            assert_eq!(rect.origin.y, solid[0].origin.y);
            assert_eq!(rect.size.y, solid[0].size.y);
        }
        let mut overlaps = [(5.0, 9.0), (8.0, 12.0), (25.0, 26.0)];
        assert_eq!(
            underline_segments(30.0, 1.0, &mut overlaps),
            vec![(0.0, 4.0), (13.0, 24.0), (27.0, 30.0)]
        );
    });
}

#[test]
fn decoration_direct_canvas_keeps_local_rounding_before_zoom_rotation_and_alpha() {
    jian_skia::with_font_lock(|| {
        let mut backend = NativeBackend::with_dpi(1.0);
        let features = BTreeMap::new();
        let r = request("Typography", &features);
        let mut actual = skia_safe::surfaces::raster_n32_premul((220, 180)).unwrap();
        let mut expected = skia_safe::surfaces::raster_n32_premul((220, 180)).unwrap();
        for canvas in [actual.canvas(), expected.canvas()] {
            canvas.clear(skia_safe::Color::WHITE);
            canvas.translate((180.25, 7.5));
            canvas.scale((-1.25, 1.5));
            canvas.rotate(13.0, None);
        }
        assert!(draw(&mut backend, actual.canvas(), &r));
        assert!(sampled_rect(
            expected.canvas(),
            Rect::xywh(4.25, 66.5625, 120.0, 0.78125),
            r.color
        ));
        assert_eq!(
            actual
                .image_snapshot()
                .peek_pixels()
                .unwrap()
                .bytes()
                .unwrap(),
            expected
                .image_snapshot()
                .peek_pixels()
                .unwrap()
                .bytes()
                .unwrap()
        );
    });
}

#[test]
fn decoration_point_samples_include_top_left_and_exclude_bottom_right() {
    let rect = Rect::xywh(4.3125, 8.5625, 21.53125, 0.78125);
    let color = Color {
        r: 0.,
        g: 0.,
        b: 0.,
        a: 0.6,
    };
    for (jx, jy) in [
        (0.625, 0.875),
        (0.375, 0.125),
        (0.125, 0.625),
        (0.875, 0.375),
    ] {
        let mut surface = skia_safe::surfaces::raster_n32_premul((64, 32)).unwrap();
        let canvas = surface.canvas();
        canvas.clear(skia_safe::Color::WHITE);
        canvas.translate((0.5 - jx, 0.5 - jy));
        canvas.scale((2., 2.));
        jian_skia::raster_sampling::with_point_sampling(|| {
            assert!(sampled_rect(canvas, rect, color));
        })
        .unwrap();
        let image = surface.image_snapshot();
        let pixels = image.peek_pixels().unwrap();
        let pixels = pixels.bytes().unwrap();
        for y in 0..32 {
            for x in 0..64 {
                let local_x = (x as f32 + jx) * 0.5;
                let local_y = (y as f32 + jy) * 0.5;
                let inside = local_x >= rect.origin.x
                    && local_x < rect.origin.x + rect.size.x
                    && local_y >= rect.origin.y
                    && local_y < rect.origin.y + rect.size.y;
                let expected = if inside { 102 } else { 255 };
                assert_eq!(
                    pixels[(y * 64 + x) * 4],
                    expected,
                    "sample{x},{y} camera{jx},{jy}"
                );
            }
        }
    }
}

#[test]
fn legacy_decoration_retains_native_rectangle_paint() {
    jian_skia::with_font_lock(|| {
        let mut backend = NativeBackend::with_dpi(1.0);
        let features = BTreeMap::new();
        let mut r = request("Typography", &features);
        r.kerning = None;
        let mut actual = skia_safe::surfaces::raster_n32_premul((140, 80)).unwrap();
        let mut expected = skia_safe::surfaces::raster_n32_premul((140, 80)).unwrap();
        actual.canvas().clear(skia_safe::Color::WHITE);
        expected.canvas().clear(skia_safe::Color::WHITE);
        assert!(draw(&mut backend, actual.canvas(), &r));
        for rect in geometry(&mut backend, &r).unwrap() {
            backend.fill_rect(expected.canvas(), rect, r.color);
        }
        assert_eq!(
            actual
                .image_snapshot()
                .peek_pixels()
                .unwrap()
                .bytes()
                .unwrap(),
            expected
                .image_snapshot()
                .peek_pixels()
                .unwrap()
                .bytes()
                .unwrap()
        );
    });
}
