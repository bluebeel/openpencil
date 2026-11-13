//! Semantic weight checks against the app's real, OFL-licensed variable font.

use super::*;
use skia_safe::{textlayout::TypefaceFontProvider, FontMgr, FourByteTag, Typeface};

const INTER: &[u8] = include_bytes!("../../../../op-host-desktop/assets/fonts/Inter-VF.ttf");
const WGHT: FourByteTag = FourByteTag::from_chars('w', 'g', 'h', 't');
const TEXT: &str = "MMMMMMMM";

fn axis(typeface: &Typeface, tag: FourByteTag) -> f32 {
    typeface
        .variation_design_position()
        .expect("variable font coordinates")
        .iter()
        .find(|coordinate| coordinate.axis == tag)
        .expect("requested axis")
        .value
}

fn resolver() -> FontResolver {
    let face = FontMgr::new()
        .new_from_data(INTER, None)
        .expect("shipped Inter variable font");
    resolver_for(face)
}

fn resolver_for(face: Typeface) -> FontResolver {
    let mut provider = TypefaceFontProvider::new();
    provider.register_typeface(face.clone(), None);
    // A private manager makes this independent of system fonts and of the
    // process-global import registry. No font is installed or substituted.
    FontResolver::with_default_typeface(provider.into(), Some(face))
}

#[test]
fn variable_weight_preserves_other_axes_and_static_font_fallback() {
    jian_skia::with_font_lock(|| {
        use skia_safe::font_arguments::{variation_position::Coordinate, VariationPosition};
        const OPSZ: FourByteTag = FourByteTag::from_chars('o', 'p', 's', 'z');
        let face = FontMgr::new().new_from_data(INTER, None).unwrap();
        let coordinates = [Coordinate {
            axis: OPSZ,
            value: 24.0,
        }];
        let arguments =
            skia_safe::FontArguments::new().set_variation_design_position(VariationPosition {
                coordinates: &coordinates,
            });
        let resolver = resolver_for(face.clone_with_arguments(&arguments).unwrap());
        let resolved = resolver
            .typeface_for_char(Some("Inter"), 'M', 700, false)
            .unwrap();
        assert_eq!(axis(&resolved.typeface, WGHT), 700.0);
        assert_eq!(
            axis(&resolved.typeface, OPSZ),
            24.0,
            "retain the selected optical size"
        );

        let face = FontMgr::new()
            .new_from_data(include_bytes!("../../../assets/Roboto-Regular.ttf"), None)
            .unwrap();
        let family = face.family_name();
        let resolver = resolver_for(face);
        let resolved = resolver
            .typeface_for_char(Some(&family), 'M', 700, false)
            .unwrap();
        assert_eq!(*resolved.typeface.font_style().weight(), 400);
        assert!(
            resolved.synthetic_bold,
            "single-weight fonts retain synthetic bold"
        );
    });
}

fn run(weight: u16) -> ShapedRun<'static> {
    ShapedRun {
        text: TEXT,
        family: "Inter",
        font_size: 40.0,
        weight,
        italic: false,
        kerning: None,
        letter_spacing: 0.0,
        font_features: None,
        line_height: 0.0,
        color: skia_safe::Color::BLACK,
    }
}

#[test]
fn variable_weight_resolves_real_outlines_without_synthetic_bold() {
    jian_skia::with_font_lock(|| {
        let resolver = resolver();
        let mut widths = Vec::new();
        // 550 guards arbitrary numeric weights, not just named CSS instances.
        for weight in [400, 500, 550, 700] {
            let face = resolver
                .typeface_for_char(Some("Inter"), 'M', weight, false)
                .expect("Inter covers Latin");
            assert_eq!(face.typeface.family_name(), "Inter");
            assert_eq!(axis(&face.typeface, WGHT), f32::from(weight));
            assert_eq!(*face.typeface.font_style().weight(), i32::from(weight));
            assert!(!face.synthetic_bold, "real variable weight {weight}");
            let segments = resolver.segment_text("Multiple letters", Some("Inter"), weight, false);
            assert_eq!(segments.len(), 1, "reuse one instance across codepoints");
            let raw = skia_safe::Font::new(&face.typeface, 40.0)
                .measure_str(TEXT, None)
                .0;
            let measured = resolver.measure_text(TEXT, 40.0, Some("Inter"), weight, false);
            assert!((raw - measured).abs() < 0.001, "measure the actual outline");
            widths.push(measured);
        }
        assert!(
            widths.windows(2).all(|pair| pair[1] > pair[0]),
            "weights change real advances: {widths:?}"
        );
        let max = resolver
            .typeface_for_char(Some("Inter"), 'M', 1000, false)
            .unwrap();
        assert_eq!(
            axis(&max.typeface, WGHT),
            900.0,
            "clamp to supported maximum"
        );
        assert!(!max.synthetic_bold);
    });
}

fn pixels(surface: &mut skia_safe::Surface) -> Vec<u8> {
    surface
        .image_snapshot()
        .peek_pixels()
        .unwrap()
        .bytes()
        .unwrap()
        .to_vec()
}

#[test]
fn paragraph_weight_matches_measured_and_painted_variable_outline() {
    jian_skia::with_font_lock(|| {
        let resolver = resolver();
        let mut shaped = ShapedText::new(&resolver);
        let mut previous_width = 0.0;
        let mut previous_ink = 0usize;
        for weight in [400, 500, 550, 700] {
            let run = run(weight);
            let mut paragraph = shaped.build(&resolver, &run);
            paragraph.layout(NATURAL_LAYOUT_BUDGET);
            let face = paragraph.get_font_at(0).typeface();
            assert_eq!(face.family_name(), "Inter");
            assert_eq!(
                axis(&face, WGHT),
                f32::from(weight),
                "Paragraph must instance wght"
            );
            let width = shaped.measure(&resolver, &run);
            assert!((width - paragraph.max_intrinsic_width()).abs() < 0.001);
            let direct = resolver.measure_text(TEXT, run.font_size, Some("Inter"), weight, false);
            assert!(
                (width - direct).abs() < 0.02,
                "measure/paint advance parity: {width} vs {direct}"
            );

            let mut actual = skia_safe::surfaces::raster_n32_premul((400, 100)).unwrap();
            actual.canvas().clear(skia_safe::Color::WHITE);
            shaped.paint(actual.canvas(), &resolver, &run, 10.0, 70.0);
            let mut expected = skia_safe::surfaces::raster_n32_premul((400, 100)).unwrap();
            expected.canvas().clear(skia_safe::Color::WHITE);
            paragraph.layout(width.ceil());
            paragraph.paint(
                expected.canvas(),
                (10.0, 70.0 - paragraph.alphabetic_baseline()),
            );
            let actual = pixels(&mut actual);
            assert_eq!(
                actual,
                pixels(&mut expected),
                "paint uses the measured variable face"
            );
            let ink = actual
                .chunks_exact(4)
                .map(|pixel| 255usize - usize::from(pixel[0]))
                .sum();
            assert!(
                width > previous_width && ink > previous_ink,
                "heavier outlines change both advance and ink"
            );
            previous_width = width;
            previous_ink = ink;
        }
    });
}
