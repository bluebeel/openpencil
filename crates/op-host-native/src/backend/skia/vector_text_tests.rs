use super::*;

fn font() -> Font {
    let face = skia_safe::FontMgr::new()
        .new_from_data(
            include_bytes!("../../../../op-host-desktop/assets/fonts/Inter-VF.ttf"),
            None,
        )
        .unwrap();
    let mut font = Font::new(face, 24.0);
    font.set_hinting(skia_safe::FontHinting::None);
    font.set_subpixel(true);
    font
}

fn render(origin: Point, modern: bool) -> (bool, Vec<u8>) {
    let mut surface = skia_safe::surfaces::raster_n32_premul((64, 48)).unwrap();
    surface.canvas().clear(Color::WHITE);
    let font = font();
    let glyphs = font.str_to_glyphs_vec("B ");
    let positions = [Point::default(), Point::new(18.0, 0.0)];
    let mut paint = Paint::default();
    paint.set_color(Color::BLACK);
    let handled = draw(
        surface.canvas(),
        &font,
        &glyphs,
        &positions,
        origin,
        &paint,
        modern,
        None,
    );
    let pixels = surface
        .image_snapshot()
        .peek_pixels()
        .unwrap()
        .bytes()
        .unwrap()
        .to_vec();
    (handled, pixels)
}

#[test]
fn vector_text_rounds_only_local_baseline_and_retains_fractional_x() {
    jian_skia::with_font_lock(|| {
        let (handled, low) = render((4.25, 32.2).into(), true);
        assert!(handled);
        assert_eq!(low, render((4.25, 32.49).into(), true).1);
        assert_ne!(low, render((4.25, 32.75).into(), true).1);
        assert_ne!(low, render((4.5, 32.2).into(), true).1);
    });
}

#[test]
fn vector_text_baseline_snap_preserves_shaped_vertical_offsets() {
    jian_skia::with_font_lock(|| {
        let font = font();
        let glyphs = font.str_to_glyphs_vec("B");
        let mut actual = skia_safe::surfaces::raster_n32_premul((64, 48)).unwrap();
        let mut expected = skia_safe::surfaces::raster_n32_premul((64, 48)).unwrap();
        let mut paint = Paint::default();
        paint.set_color(Color::BLACK);
        actual.canvas().clear(Color::WHITE);
        expected.canvas().clear(Color::WHITE);
        assert!(draw(
            actual.canvas(),
            &font,
            &glyphs,
            &[Point::new(0.0, 0.375)],
            (4.25, 32.2).into(),
            &paint,
            true,
            None
        ));
        let mut path = PathBuilder::new();
        path.add_path_with_offset(
            &units::outline(&font, glyphs[0]).unwrap_or_else(|| font.get_path(glyphs[0]).unwrap()),
            (4.25, 32.375),
            None,
        );
        assert!(draw_path(expected.canvas(), &path.detach(), &paint));
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
fn vector_text_samples_in_device_space_under_zoom_rotation_and_opacity() {
    for grid in [1, 2] {
        let mut surface = skia_safe::surfaces::raster_n32_premul((48, 48)).unwrap();
        let canvas = surface.canvas();
        canvas.clear(Color::WHITE);
        canvas.clip_rect(
            Rect::from_xywh(0.0, 0.0, 37.0, 40.0),
            skia_safe::ClipOp::Intersect,
            false,
        );
        canvas.translate((12.25, 7.5));
        canvas.scale((2.0, 1.5));
        canvas.rotate(17.0, None);
        let path = Path::rect(Rect::from_xywh(1.23, 2.34, 8.73, 11.14), None);
        let device = path.with_transform(&canvas.local_to_device_as_3x3());
        let mut paint = Paint::default();
        paint.set_color4f(Color4f::new(0.0, 0.0, 1.0, 0.5), None);
        if grid == 1 {
            super::super::sampled_clip::with_four_samples(|| {
                assert!(draw_path(canvas, &path, &paint))
            });
        } else {
            assert!(draw_path(canvas, &path, &paint));
        }
        // An unbounded device mask independently checks the bounded mask's
        // inverse-CTM shader alignment, using the rasterizer's own edge rules.
        let info = ImageInfo::new(
            (48 * grid, 48 * grid),
            ColorType::RGBAF16,
            AlphaType::Premul,
            None,
        );
        let mut reference = skia_safe::surfaces::raster(&info, None, None).unwrap();
        reference.canvas().clear(Color::TRANSPARENT);
        let mut coverage = Paint::default();
        coverage.set_color4f(Color4f::new(1.0, 1.0, 1.0, 0.25), None);
        coverage.set_blend_mode(BlendMode::Plus);
        reference.canvas().scale((grid as f32, grid as f32));
        for (sx, sy) in [
            (0.375, 0.125),
            (0.875, 0.375),
            (0.125, 0.625),
            (0.625, 0.875),
        ] {
            let saved = reference.canvas().save();
            reference
                .canvas()
                .translate(((0.5 - sx) / grid as f32, (0.5 - sy) / grid as f32));
            reference.canvas().draw_path(&device, &coverage);
            reference.canvas().restore_to_count(saved);
        }
        let sampling = if grid == 2 {
            SamplingOptions::new(skia_safe::FilterMode::Linear, skia_safe::MipmapMode::None)
        } else {
            SamplingOptions::default()
        };
        let shader = reference
            .image_snapshot()
            .to_shader(
                None,
                sampling,
                &Matrix::scale((1.0 / grid as f32, 1.0 / grid as f32)),
            )
            .unwrap();
        let mut expected = skia_safe::surfaces::raster_n32_premul((48, 48)).unwrap();
        expected.canvas().clear(Color::WHITE);
        expected.canvas().clip_rect(
            Rect::from_xywh(0.0, 0.0, 37.0, 40.0),
            skia_safe::ClipOp::Intersect,
            false,
        );
        expected
            .canvas()
            .clip_shader(shader, skia_safe::ClipOp::Intersect);
        expected.canvas().draw_paint(&paint);
        let expected = expected.image_snapshot();
        let expected = expected.peek_pixels().unwrap();
        let image = surface.image_snapshot();
        let pixels = image.peek_pixels().unwrap();
        for y in 0..48 {
            for x in 0..48 {
                let expected = expected.get_color((x, y));
                let color = pixels.get_color((x, y));
                assert!(
                    (i16::from(color.r()) - i16::from(expected.r())).abs() <= 1,
                    "device coverage at {x},{y}, grid{grid}: {color:?} != {expected:?}"
                );
                assert_eq!(color.b(), 255, "original paint applies once");
            }
        }
    }
}

#[test]
fn vector_text_legacy_and_bounded_fallback_leave_canvas_untouched() {
    jian_skia::with_font_lock(|| {
        let (handled, pixels) = render((4.25, 32.2).into(), false);
        assert!(!handled);
        assert!(pixels.iter().all(|v| *v == 255));
        let mut surface = skia_safe::surfaces::raster_n32_premul((9000, 4)).unwrap();
        surface.canvas().clear(Color::WHITE);
        let path = Path::rect(Rect::from_xywh(0.0, 0.0, 8999.0, 3.0), None);
        assert!(!draw_path(surface.canvas(), &path, &Paint::default()));
        assert!(surface
            .image_snapshot()
            .peek_pixels()
            .unwrap()
            .bytes()
            .unwrap()
            .iter()
            .all(|v| *v == 255));
    });
}

#[test]
fn vector_text_two_times_masks_retain_original_allocation_caps() {
    assert_eq!(mask_scale(4096, 1024, 2), 2);
    assert_eq!(
        mask_scale(4097, 1024, 2),
        1,
        "actual mask edge would exceed8192"
    );
    assert_eq!(
        mask_scale(4096, 1025, 2),
        1,
        "actual mask area would exceed16Mi pixels"
    );
    assert_eq!(
        mask_scale(8192, 2, 2),
        1,
        "large runs retain sixteen1x samples"
    );
    assert_eq!(
        mask_scale(512, 512, 1),
        1,
        "nested four-sample passes retain their device grid"
    );
}

#[test]
fn vector_text_curve_coverage_matches_independent_two_times_point_rasters() {
    let path = Path::oval(Rect::from_xywh(7.25, 3.5, 51.5, 51.5), None);
    let mut actual = skia_safe::surfaces::raster_n32_premul((64, 64)).unwrap();
    let mut paint = Paint::default();
    paint.set_color(Color::WHITE);
    assert!(draw_path(actual.canvas(), &path, &paint));
    let mut counts = vec![0_u8; 64 * 64];
    for (sx, sy) in [
        (0.625, 0.875),
        (0.375, 0.125),
        (0.125, 0.625),
        (0.875, 0.375),
    ] {
        let mut points = skia_safe::surfaces::raster_n32_premul((128, 128)).unwrap();
        points.canvas().translate((0.5 - sx, 0.5 - sy));
        points.canvas().scale((2.0, 2.0));
        points.canvas().draw_path(&path, &paint);
        let pixels = points.image_snapshot();
        let pixels = pixels.peek_pixels().unwrap();
        for y in 0..128 {
            for x in 0..128 {
                counts[(y / 2 * 64 + x / 2) as usize] +=
                    u8::from(pixels.get_color((x, y)).a() == 255);
            }
        }
    }
    let image = actual.image_snapshot();
    let pixels = image.peek_pixels().unwrap();
    for y in 0..64 {
        for x in 0..64 {
            let expected = (f32::from(counts[(y * 64 + x) as usize]) * 255.0 / 16.0).round() as i16;
            let alpha = i16::from(pixels.get_color((x, y)).a());
            assert!(
                (alpha - expected).abs() <= 1,
                "coverage at{x},{y}: {alpha} != {expected}"
            );
        }
    }
}
