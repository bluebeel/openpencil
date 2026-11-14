use super::*;

fn glyph(flags: &[u8]) -> Vec<u8> {
    let mut data = vec![0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
    data.extend_from_slice(&((flags.len() - 1) as u16).to_be_bytes());
    data.extend_from_slice(&[0, 0]);
    data.extend(flags.iter().map(|flag| flag | 0x30));
    data
}
fn quadratic_path() -> Path {
    let mut b = PathBuilder::new();
    b.move_to((0.5, 0.5));
    b.quad_to((2.25, 1.5), (3.5, 3.0));
    b.quad_to((4.75, 4.5), (6.5, 6.5));
    b.close();
    b.detach()
}

#[test]
fn font_units_distinguish_authored_midpoints_and_preserve_signed_rounding() {
    let input = quadratic_path();
    let implied = parse_glyph(&glyph(&[1, 0, 0, 1])).unwrap();
    let explicit = parse_glyph(&glyph(&[1, 0, 1, 0, 1])).unwrap();
    let a = normalize(&input, &implied).unwrap();
    let b = normalize(&input, &explicit).unwrap();
    let a: Vec<_> = a.iter().map(|record| record.points().to_vec()).collect();
    let b: Vec<_> = b.iter().map(|record| record.points().to_vec()).collect();
    assert_eq!(a[0][0], Point::new(1.0, 0.0));
    assert_eq!(a[1][1], Point::new(2.0, 1.0));
    assert_eq!(a[1][2], Point::new(3.0, 2.0));
    assert_eq!(
        b[1][2],
        Point::new(4.0, 3.0),
        "coincident explicit midpoint must not be guessed implicit"
    );
    assert_eq!(a[2][2], Point::new(7.0, 6.0));
    assert_eq!(rounded(Point::new(-2.5, -2.5)), Point::new(-2.0, -3.0));
    assert_eq!(
        midpoint(Point::new(-2.0, -2.0), Point::new(-5.0, -5.0)),
        Point::new(-3.0, -3.0),
        "midpoint truncates toward zero"
    );
}

#[test]
fn font_units_close_contours_with_an_implied_first_point() {
    let metadata = parse_glyph(&glyph(&[0, 0, 0, 0])).unwrap();
    let controls = [
        Point::new(2.25, -0.5),
        Point::new(5.25, -2.25),
        Point::new(3.5, -6.25),
        Point::new(-1.25, -4.75),
    ];
    let mut b = PathBuilder::new();
    b.move_to((0.5, -2.625));
    for i in 0..4 {
        let a = controls[i];
        let c = controls[(i + 1) % 4];
        b.quad_to(a, ((a.x + c.x) * 0.5, (a.y + c.y) * 0.5));
    }
    b.close();
    let result = normalize(&b.detach(), &metadata).unwrap();
    let records: Vec<_> = result
        .iter()
        .map(|record| record.points().to_vec())
        .collect();
    assert_eq!(records[0][0], Point::new(0.0, -3.0));
    assert_eq!(records[4][2], records[0][0]);
}

#[test]
fn font_units_validate_repeats_coordinates_and_unsupported_outline_shapes() {
    let mut repeat = glyph(&[1, 1, 1]);
    repeat.truncate(14);
    repeat.extend_from_slice(&[0x39, 2]);
    assert!(parse_glyph(&repeat).is_some());
    *repeat.last_mut().unwrap() = 3;
    assert!(parse_glyph(&repeat).is_none());
    let mut coordinates = glyph(&[1, 1]);
    coordinates[14] = 1;
    assert!(
        parse_glyph(&coordinates).is_none(),
        "two-byte x/y fields are missing"
    );
    let mut composite = glyph(&[1, 1]);
    composite[..2].copy_from_slice(&(-1_i16).to_be_bytes());
    assert!(parse_glyph(&composite).is_none());
    let metadata = parse_glyph(&glyph(&[1, 0, 0, 1])).unwrap();
    assert!(normalize(&Path::circle((0.0, 0.0), 10.0, None), &metadata).is_none());
}

#[test]
fn font_units_map_size_horizontal_scale_and_synthetic_skew_after_rounding() {
    jian_skia::with_font_lock(|| {
        let face = skia_safe::FontMgr::new()
            .new_from_data(
                include_bytes!("../../../../op-host-desktop/assets/fonts/Inter-VF.ttf"),
                None,
            )
            .unwrap();
        let upem = face.units_per_em().unwrap();
        let unit_font = Font::new(&face, upem as f32);
        let glyph = unit_font.unichar_to_glyph('H' as i32);
        let metadata = read_metadata(&face, glyph).unwrap();
        let units = normalize(&unit_font.get_path(glyph).unwrap(), &metadata)
            .unwrap()
            .with_transform(&Matrix::scale((1.0 / upem as f32, 1.0 / upem as f32)));
        let mut target = Font::new(face, 32.0);
        target.set_scale_x(1.25);
        target.set_skew_x(-0.2);
        let actual = outline(&target, glyph).unwrap();
        let scale = 32.0;
        let expected = units.with_transform(&Matrix::new_all(
            scale * 1.25,
            scale * (-0.2),
            0.0,
            0.0,
            scale,
            0.0,
            0.0,
            0.0,
            1.0,
        ));
        assert_eq!(actual, expected);
        assert!(
            prefix(&target.typeface(), b"glyf", MAX_PREFIX + 1).is_none(),
            "oversized requested table prefix stays on original path"
        );
    });
}

#[test]
fn backend_metadata_lru_preserves_positive_negative_hits_bounds_and_generation() {
    let mut cache = MetadataCache::new(512, 2);
    let generation = 100;
    let metadata = Rc::new(parse_glyph(&glyph(&[1, 0, 0, 1])).unwrap());
    cache.insert(generation, (10, 1), Some(Rc::clone(&metadata)));
    cache.insert(generation, (10, 2), None);
    assert!(cache.lookup(generation, (10, 2)).unwrap().is_none());
    assert!(Rc::ptr_eq(
        cache.lookup(generation, (10, 1)).unwrap().as_ref().unwrap(),
        &metadata
    ));
    cache.insert(generation, (11, 1), None);
    assert!(
        cache.lookup(generation, (10, 2)).is_none(),
        "least-recent negative entry evicted"
    );
    assert!(cache.lookup(generation, (11, 1)).unwrap().is_none());
    assert!(cache.bytes <= 512 && cache.entries.len() <= 2);
    assert!(cache.lookup(generation + 1, (10, 1)).is_none());
    assert_eq!(cache.bytes, 0);
    assert!(cache.entries.is_empty());
    let mut tiny = MetadataCache::new(64, 2);
    tiny.insert(1, (1, 1), None);
    assert!(
        tiny.entries.is_empty(),
        "a too-small cache computes without retaining"
    );
}

#[test]
fn live_direct_and_shaped_frames_reuse_glyph_metadata_without_prefix_reads() {
    use op_editor_ui::{Color, Point2D, TextLayout};
    jian_skia::with_font_lock(|| {
        for kerning in [false, true] {
            let mut backend = crate::backend::NativeBackend::with_dpi(1.0);
            let layout = TextLayout::single_run(
                "HBHH",
                "Roboto",
                24.0,
                Color::BLACK.to_jian(),
                Point2D::ZERO,
            )
            .with_kerning(Some(kerning));
            let mut surface = skia_safe::surfaces::raster_n32_premul((100, 50)).unwrap();
            PREFIX_READS.with(|reads| reads.set(0));
            surface.canvas().clear(skia_safe::Color::WHITE);
            backend.draw_text(surface.canvas(), &layout, Point2D::new(2.25, 32.2));
            let first_reads = PREFIX_READS.with(|reads| reads.get());
            assert!(first_reads > 0);
            let first = surface
                .image_snapshot()
                .peek_pixels()
                .unwrap()
                .bytes()
                .unwrap()
                .to_vec();
            surface.canvas().clear(skia_safe::Color::WHITE);
            backend.draw_text(surface.canvas(), &layout, Point2D::new(2.25, 32.2));
            assert_eq!(
                PREFIX_READS.with(|reads| reads.get()),
                first_reads,
                "no table-prefix reads on the next frame, kerning={kerning}"
            );
            assert_eq!(
                surface
                    .image_snapshot()
                    .peek_pixels()
                    .unwrap()
                    .bytes()
                    .unwrap(),
                first
            );
            let metadata = backend.shaped_text.metadata.borrow();
            assert!(metadata.entries.values().any(|entry| entry.value.is_some()));
            assert!(metadata.bytes <= 1024 * 1024 && metadata.entries.len() <= 2048);
        }
    });
}
