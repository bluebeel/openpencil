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
        let corners: Vec<_> = device
            .iter()
            .filter_map(|record| match record.verb() {
                skia_safe::PathVerb::Move => Some(record.points()[0]),
                skia_safe::PathVerb::Line => Some(record.points()[1]),
                _ => None,
            })
            .collect();
        let corners: [Point; 4] = corners.try_into().unwrap();
        let image = surface.image_snapshot();
        let pixels = image.peek_pixels().unwrap();
        for y in 0..48 {
            for x in 0..48 {
                let count = if x < 37 && y < 40 {
                    quad_sample_count(corners, x, y, grid)
                } else {
                    0
                };
                let coverage = count as f32 / (4 * grid * grid) as f32;
                let expected = ((1.0 - coverage * 0.5) * 255.0).round() as u8;
                let color = pixels.get_color((x, y));
                assert!(
                    (i16::from(color.r()) - i16::from(expected)).abs() <= 1,
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

// Independent convex-quad top-left reference in positive device Y. The paint
// is checked separately; no production stencil/row-span code is called here.
fn quad_sample_count(corners: [Point; 4], x: i32, y: i32, grid: i32) -> u32 {
    let mut count = 0;
    for (jx, jy) in [
        (0.625, 0.875),
        (0.375, 0.125),
        (0.125, 0.625),
        (0.875, 0.375),
    ] {
        let mut q = corners.map(|p| {
            [
                ((grid as f32 * p.x - jx) * 16.0).round_ties_even() as i64,
                ((grid as f32 * p.y - jy) * 16.0).round_ties_even() as i64,
            ]
        });
        let cross = |a: [i64; 2], b: [i64; 2], p: [i64; 2]| {
            (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
        };
        let area = cross(q[0], q[1], q[2]);
        if area == 0 {
            continue;
        }
        if area < 0 {
            q.reverse();
        }
        for dy in 0..grid {
            for dx in 0..grid {
                let sample = [
                    i64::from((grid * x + dx) * 16),
                    i64::from((grid * y + dy) * 16),
                ];
                let inside = (0..4).all(|i| {
                    let a = q[i];
                    let b = q[(i + 1) % 4];
                    let edge = cross(a, b, sample);
                    edge > 0 || (edge == 0 && (b[1] < a[1] || (b[1] == a[1] && b[0] > a[0])))
                });
                count += u32::from(inside);
            }
        }
    }
    count
}

#[test]
fn vector_text_line_only_outlines_keep_coverage_when_a_disjoint_curve_is_added() {
    let mut m = PathBuilder::new();
    m.move_to((4.25, 40.0));
    for p in [
        (4.25, 9.25),
        (8.5, 9.25),
        (18.75, 29.75),
        (29.0, 9.25),
        (33.25, 9.25),
        (33.25, 40.0),
        (29.0, 40.0),
        (29.0, 18.5),
        (20.5, 36.25),
        (17.0, 36.25),
        (8.5, 18.5),
        (8.5, 40.0),
    ] {
        m.line_to(p);
    }
    m.close();
    for flat in [
        Path::rect(Rect::from_xywh(4.25, 11.03125, 2.3125, 28.25), None),
        m.detach(),
    ] {
        let mut mixed = PathBuilder::new();
        mixed.add_path(&flat, None);
        mixed.move_to((1024.0, 10.0));
        mixed.quad_to((1030.0, -5.0), (1040.0, 10.0));
        mixed.close();
        let render = |path: &Path| {
            let mut surface = skia_safe::surfaces::raster_n32_premul((64, 64)).unwrap();
            let mut paint = Paint::default();
            paint.set_color(Color::WHITE);
            assert!(draw_path(surface.canvas(), path, &paint));
            surface
                .image_snapshot()
                .peek_pixels()
                .unwrap()
                .bytes()
                .unwrap()
                .to_vec()
        };
        assert_eq!(
            render(&flat),
            render(&mixed.detach()),
            "unrelated curve changes line-only coverage"
        );
    }
}
