use super::*;
use std::cell::Cell;

fn paint(canvas: &Canvas) {
    let mut paint = skia_safe::Paint::default();
    paint.set_color(skia_safe::Color::from_argb(128, 32, 96, 192));
    canvas.draw_rect(skia_safe::Rect::from_xywh(4.0, 4.0, 24.0, 16.0), &paint);
}

#[test]
fn file_and_memory_exports_encode_identical_bytes_and_call_paint_once() {
    let calls = Cell::new(0);
    let formats = [RasterFormat::Png, RasterFormat::Jpeg].into_iter().chain(
        cfg!(any(
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        ))
        .then_some(RasterFormat::Webp),
    );
    for format in formats {
        let path = std::env::temp_dir().join(format!(
            "op-encode-buffer-{}-{format:?}",
            std::process::id()
        ));
        let before = calls.get();
        let bytes = render_raster_bytes(
            Rect::xywh(0.0, 0.0, 32.0, 24.0),
            format,
            1.0,
            0.0,
            |canvas| {
                calls.set(calls.get() + 1);
                paint(canvas);
            },
        )
        .unwrap();
        render_raster(
            Rect::xywh(0.0, 0.0, 32.0, 24.0),
            &path,
            format,
            1.0,
            0.0,
            |canvas| {
                calls.set(calls.get() + 1);
                paint(canvas);
            },
        )
        .unwrap();
        assert_eq!(calls.get(), before + 2);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[test]
fn webp_encodes_native_and_correlated_with_dimensions_and_lossless_alpha() {
    for sampling in [RasterSampling::Native, RasterSampling::Correlated4x2] {
        let bounds = Rect::xywh(0.0, 0.0, 32.0, 24.0);
        let encoded = if sampling == RasterSampling::Native {
            render_raster_bytes(bounds, RasterFormat::Webp, 1.0, 0.0, paint).unwrap()
        } else {
            correlated_raster::render(bounds, RasterFormat::Webp, 1.0, 0.0, &mut paint).unwrap()
        };
        assert_eq!(&encoded[..4], b"RIFF");
        assert_eq!(&encoded[8..12], b"WEBP");
        let image = webp::Decoder::new(&encoded).decode().unwrap();
        assert_eq!(
            (image.width(), image.height(), image.is_alpha()),
            (32, 24, true)
        );
        assert_eq!(image[3], 0);
        assert_eq!(image[(12 * 32 + 16) * 4 + 3], 128);
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[test]
fn webp_codec_dimensions_refuse_before_allocating_or_painting() {
    let called = Cell::new(false);
    let error = render_raster_bytes(
        Rect::xywh(0.0, 0.0, 16_384.0, 1.0),
        RasterFormat::Webp,
        1.0,
        0.0,
        |_| called.set(true),
    )
    .unwrap_err();
    assert_eq!(
        error,
        ExportError::WebpOutputTooLarge {
            width_px: 16_384,
            height_px: 1
        }
    );
    assert!(!called.get());
}
