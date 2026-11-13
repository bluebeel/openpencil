//! Authored fractional origins and unhinted Latin outline geometry.
use super::*;
use skia_safe::{textlayout::TypefaceFontProvider, FontMgr};

fn backend() -> NativeBackend {
    let face = FontMgr::new()
        .new_from_data(
            include_bytes!("../../../../op-host-desktop/assets/fonts/Inter-VF.ttf"),
            None,
        )
        .expect("shipped OFL Inter font");
    let mut provider = TypefaceFontProvider::new();
    provider.register_typeface(face.clone(), None);
    let mut backend = NativeBackend::with_dpi(1.0);
    backend.font_resolver =
        jian_skia::FontResolver::with_default_typeface(provider.into(), Some(face));
    backend
}

fn draw(backend: &mut NativeBackend, text: &str, size: f32, origin: Point2D) -> Vec<u8> {
    let mut surface = skia_safe::surfaces::raster_n32_premul((300, 110)).unwrap();
    surface.canvas().clear(skia_safe::Color::WHITE);
    let layout = TextLayout::single_run(
        text,
        "Inter",
        size,
        op_editor_ui::Color::BLACK.to_jian(),
        Point2D::ZERO,
    )
    .with_font_weight(700);
    backend.draw_text(surface.canvas(), &layout, origin);
    surface
        .image_snapshot()
        .peek_pixels()
        .unwrap()
        .bytes()
        .unwrap()
        .to_vec()
}

#[test]
fn latin_fractional_origin_changes_coverage_without_changing_advance() {
    jian_skia::with_font_lock(|| {
        let mut backend = backend();
        for size in [15.0, 72.0] {
            let before = backend.measure_text_family_styled("Bend", size, "Inter", 700, false);
            let integer = draw(&mut backend, "Bend", size, Point2D::new(16.0, 85.0));
            let quarter = draw(&mut backend, "Bend", size, Point2D::new(16.25, 85.0));
            assert!(
                integer != quarter,
                "fractional authored x must affect Latin coverage at {size}px"
            );
            let after = backend.measure_text_family_styled("Bend", size, "Inter", 700, false);
            assert_eq!(
                before, after,
                "raster positioning must preserve layout advances"
            );
        }
    });
}

fn ink_bounds(pixels: &[u8]) -> (usize, usize, usize, usize) {
    let mut bounds = (usize::MAX, usize::MAX, 0, 0);
    for (index, pixel) in pixels.chunks_exact(4).enumerate() {
        if pixel[0] < 128 {
            let (x, y) = (index % 300, index / 300);
            bounds = (
                bounds.0.min(x),
                bounds.1.min(y),
                bounds.2.max(x),
                bounds.3.max(y),
            );
        }
    }
    assert_ne!(bounds.0, usize::MAX, "glyph has visible ink");
    bounds
}

fn nominal_grid_width(backend: &mut NativeBackend, text: &str, size: f32) -> f32 {
    let resolved = backend
        .font_resolver
        .typeface_for_char(Some("Inter"), 'A', 700, false)
        .unwrap();
    let mut font = skia_safe::Font::new(&resolved.typeface, size);
    font.set_hinting(skia_safe::FontHinting::None);
    font.set_subpixel(true);
    let glyphs = font.str_to_glyphs_vec(text);
    let mut widths = vec![0.0; glyphs.len()];
    font.get_widths(&glyphs, &mut widths);
    widths
        .into_iter()
        .map(|width| (width * 1024.0 / size).round() * size / 1024.0)
        .sum()
}

#[test]
fn latin_cap_horizontal_bounds_follow_the_selected_font_outline() {
    jian_skia::with_font_lock(|| {
        let mut backend = backend();
        let actual = draw(&mut backend, "B", 72.0, Point2D::new(16.0, 85.0));
        let resolved = backend
            .font_resolver
            .typeface_for_char(Some("Inter"), 'B', 700, false)
            .unwrap();
        let font = skia_safe::Font::new(&resolved.typeface, 72.0);
        let path = font.get_path(font.unichar_to_glyph('B' as i32)).unwrap();
        let mut surface = skia_safe::surfaces::raster_n32_premul((300, 110)).unwrap();
        surface.canvas().clear(skia_safe::Color::WHITE);
        surface.canvas().translate((16.0, 85.0));
        let mut paint = skia_safe::Paint::default();
        paint.set_color(skia_safe::Color::BLACK);
        paint.set_anti_alias(true);
        surface.canvas().draw_path(&path, &paint);
        let expected = surface
            .image_snapshot()
            .peek_pixels()
            .unwrap()
            .bytes()
            .unwrap()
            .to_vec();
        let actual = ink_bounds(&actual);
        let expected = ink_bounds(&expected);
        // Compare the flat vertical stem and rightmost extent. A8 glyph
        // masks and analytic path AA can differ at the top curve, while the
        // authored horizontal geometry must not acquire a grid-fitted stem.
        assert_eq!(
            (actual.0, actual.2),
            (expected.0, expected.2),
            "Latin caps retain their authored horizontal outline bounds"
        );
    });
}

#[test]
fn explicit_kerning_changes_latin_advances_and_paint_while_absence_stays_legacy() {
    jian_skia::with_font_lock(|| {
        let mut backend = backend();
        backend.shaped_text = super::super::shaped_text::ShapedText::new(&backend.font_resolver);
        let plain = backend.measure_text_family_styled("AVATAR", 40.0, "Inter", 700, false);
        let absent = backend.measure_text_family_kerned(
            "AVATAR",
            40.0,
            "Inter",
            700,
            false,
            None,
            0.0,
            &Default::default(),
        );
        let off = backend.measure_text_family_kerned(
            "AVATAR",
            40.0,
            "Inter",
            700,
            false,
            Some(false),
            0.0,
            &Default::default(),
        );
        let on = backend.measure_text_family_kerned(
            "AVATAR",
            40.0,
            "Inter",
            700,
            false,
            Some(true),
            0.0,
            &Default::default(),
        );
        let tracked = backend.measure_text_family_kerned(
            "AVATAR",
            40.0,
            "Inter",
            700,
            false,
            Some(true),
            2.0,
            &Default::default(),
        );
        assert!(
            (tracked - on - 10.0).abs() < 0.001,
            "tracking adds five gaps without removing real pair kerning"
        );
        assert_eq!(plain, absent);
        assert_eq!(off, nominal_grid_width(&mut backend, "AVATAR", 40.0));
        assert!(
            on < off - 1.0,
            "real font kerning changes pair advances: {on} vs {off}"
        );
        let paint = |backend: &mut NativeBackend, kerning, spacing| {
            let mut surface = skia_safe::surfaces::raster_n32_premul((300, 110)).unwrap();
            surface.canvas().clear(skia_safe::Color::WHITE);
            let layout = TextLayout::single_run(
                "AVATAR",
                "Inter",
                40.0,
                op_editor_ui::Color::BLACK.to_jian(),
                Point2D::ZERO,
            )
            .with_font_weight(700)
            .with_kerning(kerning)
            .with_letter_spacing(spacing);
            backend.draw_text(surface.canvas(), &layout, Point2D::new(16.0, 85.0));
            surface
                .image_snapshot()
                .peek_pixels()
                .unwrap()
                .bytes()
                .unwrap()
                .to_vec()
        };
        let legacy_pixels = paint(&mut backend, None, 0.0);
        let off_pixels = paint(&mut backend, Some(false), 0.0);
        let kerned_pixels = paint(&mut backend, Some(true), 0.0);
        let tracked_pixels = paint(&mut backend, Some(true), 2.0);
        assert!(
            ink_bounds(&tracked_pixels).2 >= ink_bounds(&kerned_pixels).2 + 9,
            "paint applies tracking to shaped glyph positions"
        );
        assert!(
            off_pixels != kerned_pixels,
            "paint uses the selected kerning"
        );
        assert!(
            ink_bounds(&kerned_pixels).2 < ink_bounds(&legacy_pixels).2,
            "the kerned paint has the measured narrower extent"
        );
    });
}

#[test]
fn font_features_explicit_off_wins_over_legacy_kerning() {
    jian_skia::with_font_lock(|| {
        let mut backend = backend();
        backend.shaped_text = super::super::shaped_text::ShapedText::new(&backend.font_resolver);
        let unkerned = nominal_grid_width(&mut backend, "AVATAR", 40.0);
        let off = std::collections::BTreeMap::from([("kern".into(), 0)]);
        let on = std::collections::BTreeMap::from([("kern".into(), 1)]);
        let explicit_off = backend.measure_text_family_kerned(
            "AVATAR",
            40.0,
            "Inter",
            700,
            false,
            Some(true),
            0.0,
            &off,
        );
        let legacy_off = backend.measure_text_family_kerned(
            "AVATAR",
            40.0,
            "Inter",
            700,
            false,
            Some(false),
            0.0,
            &on,
        );
        let explicit_on =
            backend.measure_text_family_kerned("AVATAR", 40.0, "Inter", 700, false, None, 0.0, &on);
        assert!((explicit_off - unkerned).abs() < 0.002);
        assert!((legacy_off - unkerned).abs() < 0.002);
        assert!(
            explicit_on < explicit_off - 1.0,
            "generic feature maps reach shaping"
        );
    });
}

#[test]
fn explicit_off_grid_matches_positioned_nominal_glyphs_and_retains_legacy_gate() {
    jian_skia::with_font_lock(|| {
        let mut backend = backend();
        let (text, size, tracking) = ("Bend your choice", 15.0, -0.3);
        let plain = backend.measure_text_family_styled(text, size, "Inter", 700, false);
        assert_eq!(
            backend.measure_text_family_kerned(
                text,
                size,
                "Inter",
                700,
                false,
                None,
                0.0,
                &Default::default()
            ),
            plain,
            "legacy documents retain their full-precision advances"
        );
        let expected_width = nominal_grid_width(&mut backend, text, size)
            + (text.chars().count() - 1) as f32 * tracking;
        let measured = backend.measure_text_family_kerned(
            text,
            size,
            "Inter",
            700,
            false,
            Some(false),
            tracking,
            &Default::default(),
        );
        assert!((measured - expected_width).abs() < 0.0001);

        let mut surface = skia_safe::surfaces::raster_n32_premul((300, 110)).unwrap();
        surface.canvas().clear(skia_safe::Color::WHITE);
        let layout = TextLayout::single_run(
            text,
            "Inter",
            size,
            op_editor_ui::Color::BLACK.to_jian(),
            Point2D::ZERO,
        )
        .with_font_weight(700)
        .with_kerning(Some(false))
        .with_letter_spacing(tracking);
        backend.draw_text(surface.canvas(), &layout, Point2D::new(16.25, 85.0));
        let actual = surface
            .image_snapshot()
            .peek_pixels()
            .unwrap()
            .bytes()
            .unwrap()
            .to_vec();

        surface.canvas().clear(skia_safe::Color::WHITE);
        let resolved = backend
            .font_resolver
            .typeface_for_char(Some("Inter"), 'B', 700, false)
            .unwrap();
        let mut font = skia_safe::Font::new(&resolved.typeface, size);
        font.set_hinting(skia_safe::FontHinting::None);
        font.set_subpixel(true);
        let glyphs = font.str_to_glyphs_vec(text);
        let mut widths = vec![0.0; glyphs.len()];
        font.get_widths(&glyphs, &mut widths);
        let mut cursor = 0.0;
        let positions: Vec<_> = widths
            .into_iter()
            .map(|width| {
                let point = skia_safe::Point::new(cursor, 0.0);
                cursor += (width * 1024.0 / size).round() * size / 1024.0 + tracking;
                point
            })
            .collect();
        let mut paint = skia_safe::Paint::default();
        paint.set_color(skia_safe::Color::BLACK);
        paint.set_anti_alias(true);
        surface.canvas().draw_glyphs_at(
            &glyphs,
            positions.as_slice(),
            (16.25, 85.0),
            &font,
            &paint,
        );
        let expected = surface
            .image_snapshot()
            .peek_pixels()
            .unwrap()
            .bytes()
            .unwrap()
            .to_vec();
        let (actual, expected) = (ink_bounds(&actual), ink_bounds(&expected));
        assert!((actual.0 as isize - expected.0 as isize).abs() <= 1);
        assert!(
            (actual.2 as isize - expected.2 as isize).abs() <= 1,
            "outline coverage retains the tracked nominal grid span"
        );
    });
    assert!(!uses_advance_grid("AV", None, None));
    assert!(uses_advance_grid("AV", Some(false), None));
    assert!(!uses_advance_grid("a\u{301}", Some(false), None));
    assert!(!uses_advance_grid("a\u{200d}b", Some(false), None));
}
