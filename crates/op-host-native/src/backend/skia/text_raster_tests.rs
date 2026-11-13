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
