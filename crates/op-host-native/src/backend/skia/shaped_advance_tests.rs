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

#[test]
fn common_ligature_tracking_uses_shaped_clusters_for_paint_and_measure() {
    jian_skia::with_font_lock(|| {
        let face = FontMgr::new()
            .new_from_data(
                include_bytes!("../../../../op-host-desktop/assets/fonts/SpaceGrotesk-VF.ttf"),
                None,
            )
            .unwrap();
        let mut provider = TypefaceFontProvider::new();
        provider.register_typeface(face.clone(), Some("Cluster Proof"));
        let resolver = FontResolver::with_default_typeface(provider.into(), Some(face));
        let mut shaped = ShapedText::new(&resolver);
        let features = std::collections::BTreeMap::from([("liga".to_string(), 1)]);
        let mut run = ShapedRun {
            text: "ffi fl fi",
            family: "Cluster Proof",
            font_size: 32.,
            weight: 400,
            italic: false,
            kerning: Some(true),
            letter_spacing: 0.,
            font_features: Some(&features),
            line_height: 0.,
            color: skia_safe::Color::BLACK,
            source_color: None,
        };
        let untracked = shaped.measure(&resolver, &run);
        let mut a = Vec::new();
        shaped.visit_glyphs(&resolver, &run, |_, ids, positions, x| {
            a.extend(ids.iter().zip(positions).map(|(&id, p)| (id, p.x + x)))
        });
        assert_eq!(
            a.len(),
            5,
            "shipped OFL font provides ffi/fl/fi ligatures plus spaces"
        );
        run.letter_spacing = 0.75;
        let tracked = shaped.measure(&resolver, &run);
        let mut b = Vec::new();
        shaped.visit_glyphs(&resolver, &run, |_, ids, positions, x| {
            b.extend(ids.iter().zip(positions).map(|(&id, p)| (id, p.x + x)))
        });
        assert!((tracked - untracked - 3.).abs() < 0.001);
        for (i, (a, b)) in a.iter().zip(&b).enumerate() {
            assert_eq!(a.0, b.0);
            assert!((b.1 - a.1 - i as f32 * 0.75).abs() < 0.001);
        }
        let off_features = std::collections::BTreeMap::from([("liga".to_string(), 0)]);
        run.font_features = Some(&off_features);
        run.letter_spacing = 0.;
        let off = shaped.measure(&resolver, &run);
        run.letter_spacing = 0.75;
        assert!((shaped.measure(&resolver, &run) - off - 6.).abs() < 0.001);
        run.kerning = None;
        run.font_features = None;
        run.letter_spacing = 0.;
        let legacy = shaped.measure(&resolver, &run);
        run.letter_spacing = 0.75;
        assert!(
            (shaped.measure(&resolver, &run) - legacy - 6.).abs() < 0.001,
            "legacy .op tracking contract is unchanged"
        );
    });
}
