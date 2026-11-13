//! Imported text keeps shaping context while using deterministic em advances.
use super::*;
use skia_safe::{textlayout::TypefaceFontProvider, FontMgr};

#[test]
fn shaped_advance_grid_preserves_kerning_and_adds_tracking_afterwards() {
    jian_skia::with_font_lock(|| {
        let face = FontMgr::new()
            .new_from_data(
                include_bytes!("../../../../op-host-desktop/assets/fonts/Inter-VF.ttf"),
                None,
            )
            .unwrap();
        let mut provider = TypefaceFontProvider::new();
        provider.register_typeface(face.clone(), Some("Grid Proof"));
        let resolver = FontResolver::with_default_typeface(provider.into(), Some(face));
        let mut shaped = ShapedText::new(&resolver);
        let mut run = ShapedRun {
            text: "AVATAR",
            family: "Grid Proof",
            font_size: 40.0,
            weight: 400,
            italic: false,
            kerning: Some(true),
            letter_spacing: 0.0,
            font_features: None,
            line_height: 0.0,
            color: skia_safe::Color::BLACK,
            source_color: None,
        };
        let on = shaped.measure(&resolver, &run);
        let grid = run.font_size / 1024.0;
        assert!(
            (on / grid - (on / grid).round()).abs() < 0.001,
            "modern text advances use the em grid"
        );
        run.kerning = Some(false);
        let off = shaped.measure(&resolver, &run);
        assert!(
            on < off - 1.0,
            "pair positioning survives nominal advance rounding"
        );
        run.kerning = Some(true);
        run.letter_spacing = 0.15;
        let tracked = shaped.measure(&resolver, &run);
        assert!(
            (tracked - on - 0.75).abs() < 0.001,
            "tracking follows five inter-character gaps after shaping"
        );
    });
}
