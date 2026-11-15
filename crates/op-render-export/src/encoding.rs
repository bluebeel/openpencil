//! Native encoded backing outlives pixel surfaces, avoiding a third full buffer.

use super::{ExportError, RasterFormat};
use skia_safe::{AlphaType, ColorType, Data, ImageInfo, Pixmap, Surface};

mod file;
pub(crate) use file::{write_atomic, CheckedWriter};

pub(crate) fn write_bytes(target: &std::path::Path, bytes: &[u8]) -> Result<(), ExportError> {
    write_atomic(target, |writer| {
        writer
            .write_all(bytes)
            .map_err(|error| ExportError::Write(error.to_string()))
    })
}

pub(super) fn write_surface(
    mut surface: Surface,
    target: &std::path::Path,
    format: RasterFormat,
) -> Result<(), ExportError> {
    if format == RasterFormat::Webp {
        let data = self::surface(surface, format)?;
        return file::write_atomic(target, |writer| {
            writer
                .write_all(data.as_bytes())
                .map_err(|error| ExportError::Write(error.to_string()))
        });
    }
    let pixels = surface.peek_pixels().ok_or(ExportError::SurfaceAlloc)?;
    file::write_atomic(target, |writer| {
        let mut checked = file::CheckedWriter::new(writer);
        let success = match format {
            RasterFormat::Png => {
                skia_safe::png_encoder::encode(&pixels, &mut checked, &Default::default())
            }
            RasterFormat::Jpeg => {
                let mut options = skia_safe::jpeg_encoder::Options::default();
                options.quality = format.quality();
                skia_safe::jpeg_encoder::encode(&pixels, &mut checked, &options)
            }
            RasterFormat::Webp => unreachable!(),
        };
        checked.result(success, format)
    })
}

pub(super) enum RasterData {
    Skia(Data),
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    Webp(webp::WebPMemory),
}

impl RasterData {
    pub(super) fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Skia(data) => data.as_bytes(),
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            Self::Webp(data) => data,
        }
    }
}

pub(super) fn validate_dimensions(
    format: RasterFormat,
    width_px: i64,
    height_px: i64,
) -> Result<(), ExportError> {
    if format == RasterFormat::Webp && (width_px > 16_383 || height_px > 16_383) {
        return Err(ExportError::WebpOutputTooLarge {
            width_px,
            height_px,
        });
    }
    Ok(())
}

pub(super) fn surface(
    mut surface: Surface,
    format: RasterFormat,
) -> Result<RasterData, ExportError> {
    if format == RasterFormat::Webp {
        let (width, height) = (surface.width(), surface.height());
        let row = width as usize * 4;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(row * height as usize)
            .map_err(|_| ExportError::SurfaceAlloc)?;
        rgba.resize(row * height as usize, 0);
        let info = ImageInfo::new(
            (width, height),
            ColorType::RGBA8888,
            AlphaType::Unpremul,
            None,
        );
        if !surface.read_pixels(&info, &mut rgba, row, (0, 0)) {
            return Err(ExportError::SurfaceAlloc);
        }
        // libwebp owns its own picture scratch. Release Skia pixels before
        // allocating that scratch rather than overlapping both with RGBA.
        drop(surface);
        return webp(&rgba, width, height, format.quality());
    }
    let image = surface.image_snapshot();
    let data = image
        .encode(None, format.skia(), format.quality())
        .ok_or(ExportError::Encode {
            format: format.user_label(),
        })?;
    Ok(RasterData::Skia(data))
}

pub(super) fn pixmap_bytes(pixels: &Pixmap, format: RasterFormat) -> Result<Vec<u8>, ExportError> {
    if format == RasterFormat::Webp {
        validate_dimensions(format, pixels.width() as i64, pixels.height() as i64)?;
        // Correlated resolve already supplies tightly packed straight RGBA.
        if pixels.color_type() != ColorType::RGBA8888
            || pixels.alpha_type() != AlphaType::Unpremul
            || pixels.row_bytes() != pixels.width() as usize * 4
        {
            return Err(ExportError::Encode { format: "WEBP" });
        }
        let data = webp(
            pixels.bytes().ok_or(ExportError::SurfaceAlloc)?,
            pixels.width(),
            pixels.height(),
            format.quality(),
        )?;
        return Ok(data.as_bytes().to_vec());
    }
    pixels
        .encode(format.skia(), format.quality())
        .ok_or(ExportError::Encode {
            format: format.user_label(),
        })
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn webp(rgba: &[u8], width: i32, height: i32, quality: u32) -> Result<RasterData, ExportError> {
    webp::Encoder::from_rgba(rgba, width as u32, height as u32)
        .encode_simple(false, quality as f32)
        .map(RasterData::Webp)
        .map_err(|_| ExportError::Encode { format: "WEBP" })
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn webp(_: &[u8], _: i32, _: i32, _: u32) -> Result<RasterData, ExportError> {
    Err(ExportError::UnsupportedFormat { format: "WEBP" })
}
