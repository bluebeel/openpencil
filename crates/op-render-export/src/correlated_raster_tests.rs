use super::*;
use op_editor_ui::{Color as UiColor, ImageDrawMode, ImagePaintOptions};
use op_host_native::{raster_sampling, NativeBackend};
use skia_safe::Data;
use std::cell::Cell;

fn decode(bytes: &[u8], width: i32, height: i32) -> Vec<u8> {
    let mut codec = skia_safe::Codec::from_data(Data::new_copy(bytes)).unwrap();
    let info = ImageInfo::new(
        (width, height),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        None,
    );
    let mut pixels = vec![0; width as usize * height as usize * 4];
    assert_eq!(
        codec.get_pixels_with_options(&info, &mut pixels, width as usize * 4, None),
        skia_safe::codec::Result::Success
    );
    pixels
}
fn over(bg: [u8; 4], fg: [f32; 4]) -> [u8; 4] {
    std::array::from_fn(|c| {
        let source = if c == 3 { fg[3] } else { fg[c] * fg[3] };
        let destination = (u32::from(bg[c]) * 257) as f32 * (1.0_f32 / 65535.0);
        quantize(source + destination * (1.0 - fg[3]))
    })
}
fn color(v: [u8; 3], alpha: f32) -> UiColor {
    UiColor {
        r: f32::from(v[0]) / 255.0,
        g: f32::from(v[1]) / 255.0,
        b: f32::from(v[2]) / 255.0,
        a: alpha,
    }
}
fn contains(p: (f32, f32), r: [f32; 4]) -> bool {
    p.0 >= r[0] && p.1 >= r[1] && p.0 < r[0] + r[2] && p.1 < r[1] + r[3]
}

#[test]
fn correlated_scene_resolves_colored_overlaps_image_and_clip_together() {
    const W: usize = 24;
    const H: usize = 18;
    let source: Vec<u8> = (0..64)
        .flat_map(|i| {
            [
                (i % 8 * 31) as u8,
                (i / 8 * 29) as u8,
                ((i % 8 + i / 8) % 8 * 27) as u8,
                255,
            ]
        })
        .collect();
    let info = ImageInfo::new((8, 8), ColorType::RGBA8888, AlphaType::Premul, None);
    let image = skia_safe::images::raster_from_data(&info, Data::new_copy(&source), 32).unwrap();
    let mut backend = NativeBackend::with_dpi(1.0);
    backend.install_raster_image(9001, image, u32::MAX);
    let calls = Cell::new(0);
    let mut canvas_addresses = Vec::new();
    let bytes = render(
        Rect::xywh(0.0, 0.0, W as f32, H as f32),
        RasterFormat::Png,
        1.0,
        0.0,
        &mut |canvas| {
            calls.set(calls.get() + 1);
            canvas_addresses.push(canvas as *const Canvas as usize);
            assert!(raster_sampling::is_point_sampling());
            backend.fill_rect(
                canvas,
                Rect::xywh(0.0, 0.0, W as f32, H as f32),
                color([11, 37, 83], 1.0),
            );
            backend.fill_rect(
                canvas,
                Rect::xywh(1.25, 2.125, 10.5, 9.75),
                color([190, 41, 77], 0.35),
            );
            backend.fill_rect(
                canvas,
                Rect::xywh(6.625, 1.25, 9.5, 11.0),
                color([17, 177, 63], 0.6),
            );
            let saved = canvas.save();
            backend.clip_oval(canvas, Rect::xywh(3.125, 2.625, 12.25, 12.25));
            backend.draw_image_with_paint_options(
                canvas,
                Rect::xywh(3.125, 3.875, 9.0, 8.0),
                9001,
                &[],
                &ImagePaintOptions {
                    mode: ImageDrawMode::Stretch,
                    nearest_sampling: true,
                    opacity: 0.7,
                    ..Default::default()
                },
            );
            canvas.restore_to_count(saved);
        },
    )
    .unwrap();
    assert_eq!(calls.get(), 4);
    assert!(
        canvas_addresses.iter().all(|p| *p == canvas_addresses[0]),
        "one source canvas is reused"
    );
    let actual = decode(&bytes, W as i32, H as i32);
    let mut expected = vec![0u8; W * H * 4];
    for (pass, jitter) in [
        (0.625, 0.875),
        (0.375, 0.125),
        (0.125, 0.625),
        (0.875, 0.375),
    ]
    .into_iter()
    .enumerate()
    {
        let mut samples = vec![0; W * H * 16];
        for y in 0..H * 2 {
            for x in 0..W * 2 {
                let p = ((x as f32 + jitter.0) / 2.0, (y as f32 + jitter.1) / 2.0);
                let mut c = [11, 37, 83, 255];
                for (r, rgb, alpha) in [
                    ([1.25, 2.125, 10.5, 9.75], [190, 41, 77], 0.35),
                    ([6.625, 1.25, 9.5, 11.0], [17, 177, 63], 0.6),
                ] {
                    if contains(p, r) {
                        let f = color(rgb, alpha);
                        c = over(c, [f.r, f.g, f.b, f.a]);
                    }
                }
                let dx = (p.0 - 9.25) / 6.125;
                let dy = (p.1 - 8.75) / 6.125;
                if dx * dx + dy * dy <= 1.0 && contains(p, [3.125, 3.875, 9.0, 8.0]) {
                    let ix = ((p.0 - 3.125) * 8.0 / 9.0).floor() as usize;
                    let iy = (p.1 - 3.875).floor() as usize;
                    let index = (iy * 8 + ix) * 4;
                    c = over(
                        c,
                        [
                            f32::from(source[index]) * (1.0 / 255.0),
                            f32::from(source[index + 1]) * (1.0 / 255.0),
                            f32::from(source[index + 2]) * (1.0 / 255.0),
                            0.7,
                        ],
                    );
                }
                samples[(y * W * 2 + x) * 4..(y * W * 2 + x) * 4 + 4].copy_from_slice(&c);
            }
        }
        resolve(&samples, W * 8, &mut expected, W, H, pass);
    }
    let errors: Vec<_> = actual
        .chunks_exact(4)
        .zip(expected.chunks_exact(4))
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, (a, b))| ((i % W, i / W), a, b))
        .collect();
    assert!(
        errors.is_empty(),
        "{} pixels differ; first: {:?}",
        errors.len(),
        &errors[..errors.len().min(8)]
    );
    assert!(!raster_sampling::is_point_sampling());
}

#[test]
fn correlated_working_set_checks_actual_source_and_accumulator_before_paint() {
    assert_eq!(
        dimensions(Rect::xywh(0.0, 0.0, 1920.0, 1080.0), 1.0, 0.0).unwrap(),
        (1920, 1080)
    );
    let calls = Cell::new(0);
    let error = render(
        Rect::xywh(0.0, 0.0, 4096.0, 4096.0),
        RasterFormat::Png,
        1.0,
        0.0,
        &mut |_| calls.set(1),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ExportError::CorrelatedWorkingSet {
            requested_bytes: 335544320,
            max_bytes: MAX_CORRELATED_WORKING_BYTES
        }
    ));
    assert_eq!(calls.get(), 0);
}

#[test]
fn correlated_resolve_retains_rgba8_pass_stores_and_straight_png_bytes() {
    let mut atlas = [0u8; 4];
    for (pass, value) in [16, 45, 30, 30].into_iter().enumerate() {
        let mut pixels = [0u8; 16];
        for p in pixels.chunks_exact_mut(4) {
            p.copy_from_slice(&[value, 80, 160, 255]);
        }
        resolve(&pixels, 8, &mut atlas, 1, 1, pass);
    }
    assert_eq!(atlas, [30, 80, 160, 255]);
    let mut straight = vec![18, 36, 55, 91, 0, 0, 0, 0];
    unpremultiply(&mut straight);
    let expected = straight.clone();
    let info = ImageInfo::new((2, 1), ColorType::RGBA8888, AlphaType::Unpremul, None);
    let pixels = Pixmap::new(&info, &mut straight, 8).unwrap();
    let png = pixels
        .encode(skia_safe::EncodedImageFormat::PNG, 100)
        .unwrap();
    assert_eq!(
        decode(&png, 2, 1),
        expected,
        "PNG encoding does not re-quantize alpha edges"
    );
}

#[test]
fn correlated_legacy_callback_remains_once_and_scene_mode_matches_worker() {
    let calls = Cell::new(0);
    crate::render_raster_bytes(
        Rect::xywh(0.0, 0.0, 2.0, 2.0),
        RasterFormat::Png,
        1.0,
        0.0,
        |_| calls.set(1),
    )
    .unwrap();
    assert_eq!(calls.get(), 1);
    let scene = std::sync::Arc::new(crate::test_support::scene_with(vec![
        crate::test_support::filled_rect("box", 0.0, 0.0, 5.5, 4.25, color([29, 140, 198], 0.6)),
    ]));
    let expected = crate::render_page_raster_bytes_with_sampling(
        scene.active_page().unwrap(),
        RasterFormat::Png,
        1.0,
        RasterSampling::Correlated4x2,
    )
    .unwrap();
    let worker = crate::SharedRasterWorker::new().unwrap();
    let admission = worker.try_acquire().unwrap();
    for _ in 0..2 {
        assert_eq!(
            admission
                .render_with_sampling(
                    std::sync::Arc::clone(&scene),
                    crate::SharedRasterTarget::Page {
                        page_id: "p1".into()
                    },
                    RasterFormat::Png,
                    1.0,
                    RasterSampling::Correlated4x2
                )
                .unwrap(),
            expected
        );
    }
    assert_eq!(worker.stats().backend_initializations, 1);
}
