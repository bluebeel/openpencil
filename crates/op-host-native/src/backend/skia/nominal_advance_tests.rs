//! Independent integer hmtx controls for the bundled OFL font bytes.
use super::*;
use skia_safe::{Font, FontArguments, FontHinting, FontMgr, Typeface};

fn inter(weight: f32, optical_size: f32) -> Typeface {
    let face = FontMgr::new()
        .new_from_data(
            include_bytes!("../../../../op-host-desktop/assets/fonts/Inter-VF.ttf"),
            None,
        )
        .unwrap();
    let coordinates = [
        skia_safe::font_arguments::variation_position::Coordinate {
            axis: skia_safe::FourByteTag::from_chars('w', 'g', 'h', 't'),
            value: weight,
        },
        skia_safe::font_arguments::variation_position::Coordinate {
            axis: skia_safe::FourByteTag::from_chars('o', 'p', 's', 'z'),
            value: optical_size,
        },
    ];
    let varied = face
        .clone_with_arguments(&FontArguments::new().set_variation_design_position(
            skia_safe::font_arguments::VariationPosition {
                coordinates: &coordinates,
            },
        ))
        .unwrap();
    varied
}

fn font(face: &Typeface, size: f32) -> Font {
    let mut font = Font::new(face, size);
    font.set_hinting(FontHinting::None);
    font.set_subpixel(true);
    font
}

fn raw_advances(font: &Font, glyphs: &[skia_safe::GlyphId]) -> Vec<f32> {
    let mut widths = vec![0.0; glyphs.len()];
    font.get_widths(glyphs, &mut widths);
    widths
}

#[test]
fn nominal_advance_matches_instantiated_hmtx_and_preserves_other_axes() {
    jian_skia::with_font_lock(|| {
        // FontTools instantiateVariableFont hmtx, independently read from the
        // bundled font at these exact axes. No renderer metrics in expectations.
        for (weight, optical, units) in [
            (550.0, 14.0, [1471.0, 1471.0, 1345.0]),
            (700.0, 14.0, [1529.0, 1529.0, 1367.0]),
            (550.0, 32.0, [1399.0, 1391.0, 1275.0]),
        ] {
            let face = inter(weight, optical);
            let mut font = font(&face, 40.0);
            font.set_scale_x(1.25);
            font.set_skew_x(-0.25);
            let glyphs = font.str_to_glyphs_vec("AVT");
            let actual = jian_skia::advance_grid::nominal_advances(&font, &glyphs);
            let expected = units.map(|value| value * (1.0 / 2048.0) * 40.0 * 1.25);
            assert_eq!(
                actual, expected,
                "axes {weight}/{optical} survive UPEM lookup"
            );
        }
        let face = FontMgr::new()
            .new_from_data(
                include_bytes!("../../../../op-host-desktop/assets/fonts/DMMono-Regular.ttf"),
                None,
            )
            .unwrap();
        let font = font(&face, 16.0);
        let glyphs = font.str_to_glyphs_vec("AVT");
        let widths = jian_skia::advance_grid::nominal_advances(&font, &glyphs);
        assert_eq!(widths, vec![600.0 * 0.001 * 16.0; 3]);
        let mut synthetic = font.clone();
        synthetic.set_embolden(true);
        assert_eq!(
            jian_skia::advance_grid::nominal_advances(&synthetic, &glyphs),
            raw_advances(&synthetic, &glyphs),
            "synthetic fallback keeps its existing advance metrics"
        );
    });
}

#[test]
fn nominal_advance_keeps_shaped_pair_residuals_separate() {
    jian_skia::with_font_lock(|| {
        let font = font(&inter(550.0, 14.0), 40.0);
        let glyphs = font.str_to_glyphs_vec("AVT");
        let raw = raw_advances(&font, &glyphs);
        let pair = [-64.0, 18.0, -4.0].map(|units| units * 40.0 / 2048.0);
        let positions = [
            skia_safe::Point::new(0.0, 0.0),
            skia_safe::Point::new(raw[0] + pair[0], 0.0),
            skia_safe::Point::new(raw[0] + pair[0] + raw[1] + pair[1], 0.0),
        ];
        let advance = positions[2].x + raw[2] + pair[2];
        let adjusted = jian_skia::advance_grid::adjust_glyph_run(
            &font,
            &glyphs,
            &positions,
            &[0, 1, 2],
            advance,
        )
        .unwrap();
        let mut expected = 0.0;
        for (index, units) in [1471.0, 1471.0, 1345.0].into_iter().enumerate() {
            assert!((adjusted.positions[index].x - expected).abs() < 0.0001);
            expected +=
                quantize_advance(units * 40.0 / 2048.0, 40.0) + quantize_advance(pair[index], 40.0);
        }
        assert!((advance + adjusted.advance_correction - expected).abs() < 0.0001);
        assert!(
            jian_skia::advance_grid::adjust_glyph_run(
                &font,
                &glyphs,
                &positions,
                &[0, 0, 2],
                advance,
            )
            .is_none(),
            "joined clusters retain their established fallback"
        );
    });
}

fn resolver(face: Typeface) -> jian_skia::FontResolver {
    let mut provider = skia_safe::textlayout::TypefaceFontProvider::new();
    provider.register_typeface(face.clone(), Some("Nominal Proof"));
    jian_skia::FontResolver::with_default_typeface(provider.into(), Some(face))
}

#[test]
fn nominal_advance_shaped_measure_positions_and_tracking_agree() {
    jian_skia::with_font_lock(|| {
        let resolver = resolver(inter(550.0, 14.0));
        let mut shaped = super::super::shaped_text::ShapedText::new(&resolver);
        let run = ShapedRun {
            text: "AVATAR",
            family: "Nominal Proof",
            font_size: 40.0,
            weight: 550,
            italic: false,
            kerning: Some(false),
            letter_spacing: 0.15,
            font_features: None,
            line_height: 0.0,
            color: skia_safe::Color::BLACK,
            source_color: None,
        };
        let steps = [1471.0, 1471.0, 1471.0, 1345.0, 1471.0, 1331.0]
            .map(|units| quantize_advance(units * 40.0 / 2048.0, 40.0));
        let expected_width = steps.iter().sum::<f32>() + 0.75;
        assert!((shaped.measure(&resolver, &run) - expected_width).abs() < 0.0001);
        let mut seen = 0;
        shaped.visit_glyphs(&resolver, &run, |_, glyphs, positions, origin| {
            let mut expected = 0.0;
            for (index, position) in positions.iter().enumerate() {
                assert!((origin + position.x - expected).abs() < 0.0001);
                expected += steps[index] + 0.15;
            }
            seen += glyphs.len();
        });
        assert_eq!(seen, 6);
    });
}

#[test]
fn nominal_advance_direct_measure_and_paint_agree_without_changing_legacy() {
    jian_skia::with_font_lock(|| {
        let mut backend = NativeBackend::with_dpi(1.0);
        backend.font_resolver = resolver(inter(550.0, 14.0));
        let size = 40.0;
        let steps = [1471.0, 1471.0, 1471.0, 1345.0, 1471.0, 1331.0]
            .map(|units| quantize_advance(units * size / 2048.0, size));
        let expected_width = steps.iter().sum::<f32>() + 0.75;
        let features = Default::default();
        let measure = |backend: &mut NativeBackend, kerning| {
            backend.measure_text_family_kerned(
                "AVATAR",
                size,
                "Nominal Proof",
                550,
                false,
                kerning,
                0.15,
                &features,
            )
        };
        assert!((measure(&mut backend, Some(false)) - expected_width).abs() < 0.0001);
        let legacy =
            backend.measure_text_family_styled("AVATAR", size, "Nominal Proof", 550, false);
        assert_eq!(measure(&mut backend, None), legacy + 0.75);
        let mut surface = skia_safe::surfaces::raster_n32_premul((300, 110)).unwrap();
        surface.canvas().clear(skia_safe::Color::WHITE);
        let layout = TextLayout::single_run(
            "AVATAR",
            "Nominal Proof",
            size,
            op_editor_ui::Color::BLACK.to_jian(),
            Point2D::ZERO,
        )
        .with_font_weight(550)
        .with_kerning(Some(false))
        .with_letter_spacing(0.15);
        backend.draw_text(surface.canvas(), &layout, Point2D::new(16.25, 85.0));
        let actual = surface
            .image_snapshot()
            .peek_pixels()
            .unwrap()
            .bytes()
            .unwrap()
            .to_vec();
        surface.canvas().clear(skia_safe::Color::WHITE);
        let face = backend
            .font_resolver
            .typeface_for_char(Some("Nominal Proof"), 'A', 550, false)
            .unwrap();
        let font = font(&face.typeface, size);
        let glyphs = font.str_to_glyphs_vec("AVATAR");
        let mut x = 0.0;
        let positions = steps.map(|step| {
            let p = skia_safe::Point::new(x, 0.0);
            x += step + 0.15;
            p
        });
        let mut paint = skia_safe::Paint::default();
        paint.set_color(skia_safe::Color::BLACK);
        paint.set_anti_alias(true);
        assert!(super::super::vector_text::draw(
            surface.canvas(),
            &font,
            &glyphs,
            &positions,
            (16.25, 85.0).into(),
            &paint,
            true,
            None,
        ));
        assert_eq!(
            actual,
            surface
                .image_snapshot()
                .peek_pixels()
                .unwrap()
                .bytes()
                .unwrap()
        );
    });
}
