//! Whole-scene cameras, resolved only after backdrop compositing completes.

use super::{ExportError, RasterFormat, MAX_RASTER_SIDE_PX, MAX_RASTER_TOTAL_PX};
use op_editor_ui::Rect;
use skia_safe::{AlphaType, Canvas, Color, ColorType, ImageInfo, Pixmap};

pub const MAX_CORRELATED_WORKING_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RasterSampling {
    #[default]
    Native,
    Correlated4x2,
}
impl RasterSampling {
    pub fn parse(value: &str) -> Result<Self, ExportError> {
        match value {
            "native" => Ok(Self::Native),
            "correlated4x2" => Ok(Self::Correlated4x2),
            _ => Err(ExportError::UnknownRasterSampling {
                value: value.into(),
            }),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Correlated4x2 => "correlated4x2",
        }
    }
}

pub(crate) fn dimensions(bounds: Rect, scale: f32, margin: f32) -> Result<(i32, i32), ExportError> {
    let width = ((bounds.size.x + 2.0 * margin) * scale).round();
    let height = ((bounds.size.y + 2.0 * margin) * scale).round();
    if !width.is_finite() || !height.is_finite() {
        return Err(ExportError::NonFiniteOutputSize);
    }
    let (w, h) = ((width as i64).max(1), (height as i64).max(1));
    if w > MAX_RASTER_SIDE_PX || h > MAX_RASTER_SIDE_PX || w.saturating_mul(h) > MAX_RASTER_TOTAL_PX
    {
        return Err(ExportError::OutputTooLarge {
            width_px: w,
            height_px: h,
        });
    }
    let bytes = (w as u64)
        .checked_mul(h as u64)
        .and_then(|pixels| pixels.checked_mul(20));
    if w * 2 > MAX_RASTER_SIDE_PX || h * 2 > MAX_RASTER_SIDE_PX {
        return Err(ExportError::CorrelatedPhysicalSize {
            width_px: w * 2,
            height_px: h * 2,
            max_side_px: MAX_RASTER_SIDE_PX,
        });
    }
    if bytes.is_none_or(|bytes| bytes > MAX_CORRELATED_WORKING_BYTES) {
        return Err(ExportError::CorrelatedWorkingSet {
            requested_bytes: bytes.unwrap_or(u64::MAX),
            max_bytes: MAX_CORRELATED_WORKING_BYTES,
        });
    }
    Ok((w as i32, h as i32))
}

fn buffer(length: usize) -> Result<Vec<u8>, ExportError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| ExportError::SurfaceAlloc)?;
    bytes.resize(length, 0);
    Ok(bytes)
}

pub(crate) fn render(
    bounds: Rect,
    format: RasterFormat,
    scale: f32,
    margin: f32,
    paint: &mut dyn FnMut(&Canvas),
) -> Result<Vec<u8>, ExportError> {
    format.ensure_supported()?;
    let (width, height) = dimensions(bounds, scale, margin)?;
    // Exactly one source plus one byte accumulator; no snapshots/readback copy
    // and no four retained camera images. Effects/encoder allocations are extra.
    let source_row = width as usize * 8;
    let mut source = buffer(source_row * height as usize * 2)?;
    let mut atlas = buffer(width as usize * height as usize * 4)?;
    let info = ImageInfo::new(
        (width * 2, height * 2),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    let mut surface = skia_safe::surfaces::wrap_pixels(&info, &mut source, source_row, None)
        .ok_or(ExportError::SurfaceAlloc)?;
    for (pass, (jx, jy)) in [
        (0.625, 0.875),
        (0.375, 0.125),
        (0.125, 0.625),
        (0.875, 0.375),
    ]
    .into_iter()
    .enumerate()
    {
        let canvas = surface.canvas();
        let saved = canvas.save();
        canvas.clear(if format.supports_alpha() {
            Color::TRANSPARENT
        } else {
            Color::WHITE
        });
        canvas.translate((0.5 - jx, 0.5 - jy));
        canvas.scale((2.0 * scale, 2.0 * scale));
        canvas.translate((margin - bounds.origin.x, margin - bounds.origin.y));
        let painted = op_host_native::raster_sampling::with_point_sampling(|| paint(canvas));
        canvas.restore_to_count(saved);
        painted.map_err(ExportError::PointSampling)?;
        let pixels = surface.peek_pixels().ok_or(ExportError::SurfaceAlloc)?;
        let pixels = pixels.bytes().ok_or(ExportError::SurfaceAlloc)?;
        resolve(
            pixels,
            source_row,
            &mut atlas,
            width as usize,
            height as usize,
            pass,
        );
    }
    drop(surface);
    drop(source);
    unpremultiply(&mut atlas);
    let info = ImageInfo::new(
        (width, height),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        None,
    );
    let pixels =
        Pixmap::new(&info, &mut atlas, width as usize * 4).ok_or(ExportError::SurfaceAlloc)?;
    // Direct pixmap encoding preserves the final straight bytes; an N32
    // image encode would apply another premultiply/unpremultiply conversion.
    pixels
        .encode(format.skia(), format.quality())
        .ok_or(ExportError::Encode {
            format: format.user_label(),
        })
}

fn quantize(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round_ties_even() as u8
}
fn resolve(source: &[u8], row: usize, atlas: &mut [u8], width: usize, height: usize, pass: usize) {
    let weight = [1.0_f32, 0.5, 1.0 / 3.0, 0.25][pass];
    for y in 0..height {
        for x in 0..width {
            for channel in 0..4 {
                let offset = y * 2 * row + x * 8 + channel;
                let sum = u16::from(source[offset])
                    + u16::from(source[offset + 4])
                    + u16::from(source[offset + row])
                    + u16::from(source[offset + row + 4]);
                let incoming = f32::from(sum) * (1.0_f32 / 1020.0);
                let output = &mut atlas[(y * width + x) * 4 + channel];
                let previous = (u32::from(*output) * 257) as f32 * (1.0_f32 / 65535.0);
                let src = incoming * weight;
                let dst = previous * (1.0 - weight);
                *output = quantize(src + dst);
            }
        }
    }
}
fn unpremultiply(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = f32::from(pixel[3]) * (1.0_f32 / 255.0);
        for value in &mut pixel[..3] {
            *value = if alpha == 0.0 {
                0
            } else {
                quantize((f32::from(*value) * (1.0_f32 / 255.0)) / alpha)
            };
        }
    }
}

#[cfg(test)]
#[path = "correlated_raster_tests.rs"]
mod tests;
