use super::*;
use std::cell::Cell;

fn font() -> Font {
    let face = skia_safe::FontMgr::new()
        .new_from_data(
            include_bytes!("../../../../op-host-desktop/assets/fonts/Inter-VF.ttf"),
            None,
        )
        .unwrap();
    Font::new(face, 24.0)
}

#[test]
fn export_outline_reuse_exact_keys_scope_and_font_generation() {
    jian_skia::with_font_lock(|| {
        let font = font();
        let glyphs = font.str_to_glyphs_vec("AV");
        let positions = [Point::new(0.0, 0.0), Point::new(17.25, 0.0)];
        let origin = Point::new(1.25, 24.2);
        let calls = Cell::new(0);
        let path = || {
            calls.set(calls.get() + 1);
            Some(Path::rect(
                skia_safe::Rect::from_xywh(0.0, 0.0, 8.0, 9.0),
                None,
            ))
        };
        with_scope(|| {
            for _ in 0..4 {
                outline(&font, &glyphs, &positions, origin, path).unwrap();
            }
            assert_eq!(calls.get(), 1);
            with_scope(|| {
                outline(&font, &glyphs, &positions, origin, path).unwrap();
            });
            assert_eq!(calls.get(), 1, "nested scope shares outer cache");
            outline(
                &font.with_size(25.0).unwrap(),
                &glyphs,
                &positions,
                origin,
                path,
            )
            .unwrap();
            outline(
                &font,
                &glyphs,
                &[positions[0], Point::new(17.5, 0.0)],
                origin,
                path,
            )
            .unwrap();
            outline(&font, &glyphs, &positions, Point::new(1.5, 24.2), path).unwrap();
            outline(&font, &[glyphs[1], glyphs[0]], &positions, origin, path).unwrap();
            assert_eq!(calls.get(), 5);
            with_cache(|cache| {
                let generation = cache.generation;
                cache.refresh(generation.wrapping_add(1));
                assert!(cache.outlines.is_empty() && cache.flattened.is_empty());
                assert_eq!(cache.bytes, 0);
                assert_eq!(cache.entries, 0);
            });
            outline(&font, &glyphs, &positions, origin, path).unwrap();
            assert_eq!(calls.get(), 6);
        });
        assert!(ACTIVE.with(|slot| slot.borrow().is_none()));
        outline(&font, &glyphs, &positions, origin, path).unwrap();
        assert_eq!(calls.get(), 7, "no retained reuse after export");
    });
}

#[test]
fn export_subdivision_reuse_ignores_only_translation_and_is_bounded() {
    let path = Path::rect(skia_safe::Rect::from_xywh(0.0, 0.0, 8.0, 9.0), None);
    let calls = Cell::new(0);
    let build = || {
        calls.set(calls.get() + 1);
        Some(path.clone())
    };
    with_scope(|| {
        for (x, y) in [(0.0, 0.0), (0.25, -0.375), (-400.0, 200.0)] {
            flattened(
                &path,
                &Matrix::new_all(2.0, 0.0, x, 0.0, 2.0, y, 0.0, 0.0, 1.0),
                build,
            )
            .unwrap();
        }
        assert_eq!(calls.get(), 1);
        flattened(&path, &Matrix::scale((3.0, 2.0)), build).unwrap();
        flattened(
            &Path::rect(skia_safe::Rect::from_xywh(0.0, 0.0, 9.0, 9.0), None),
            &Matrix::scale((2.0, 2.0)),
            build,
        )
        .unwrap();
        assert_eq!(calls.get(), 3);
        with_cache(|cache| {
            cache.max_bytes = 0;
            cache.max_entries = 0;
            cache.flattened.clear();
            cache.outlines.clear();
            cache.bytes = 0;
            cache.entries = 0;
        });
        for _ in 0..2 {
            flattened(&path, &Matrix::new_identity(), build).unwrap();
        }
        assert_eq!(calls.get(), 5, "full budget computes without retaining");
        with_cache(|cache| {
            assert_eq!(cache.bytes, 0);
            assert_eq!(cache.entries, 0);
        });
    });
    let panic = std::panic::catch_unwind(|| with_scope(|| panic!("scope test")));
    assert!(panic.is_err());
    assert!(ACTIVE.with(|slot| slot.borrow().is_none()));
}

#[test]
fn export_text_reuse_keeps_per_camera_paint_stroke_and_positions_exact() {
    use skia_safe::{Color, Paint, PaintStyle};
    jian_skia::with_font_lock(|| {
        let font = font();
        let glyphs = font.str_to_glyphs_vec("AVg");
        let positions = [
            Point::new(0.0, 0.0),
            Point::new(17.25, 0.25),
            Point::new(34.0, 0.0),
        ];
        let render = || {
            let mut outputs = Vec::new();
            for (index, (jx, jy)) in [
                (0.625, 0.875),
                (0.375, 0.125),
                (0.125, 0.625),
                (0.875, 0.375),
            ]
            .into_iter()
            .enumerate()
            {
                let mut surface = skia_safe::surfaces::raster_n32_premul((144, 96)).unwrap();
                let canvas = surface.canvas();
                canvas.clear(Color::WHITE);
                canvas.translate((0.5 - jx, 0.5 - jy));
                canvas.scale((2.0, 2.0));
                let mut paint = Paint::default();
                paint.set_color4f(
                    skia_safe::Color4f::new(index as f32 / 4.0, 0.2, 0.8, 0.6),
                    None,
                );
                if index % 2 != 0 {
                    paint.set_style(PaintStyle::StrokeAndFill);
                    paint.set_stroke_width(0.4);
                }
                jian_skia::raster_sampling::with_point_sampling(|| {
                    assert!(super::super::draw(
                        canvas,
                        &font,
                        &glyphs,
                        &positions,
                        Point::new(4.25, 32.2),
                        &paint,
                        true
                    ));
                })
                .unwrap();
                outputs.push(
                    surface
                        .image_snapshot()
                        .peek_pixels()
                        .unwrap()
                        .bytes()
                        .unwrap()
                        .to_vec(),
                );
            }
            outputs
        };
        let expected = render();
        let actual = with_scope(render);
        assert_eq!(actual, expected);
    });
}
