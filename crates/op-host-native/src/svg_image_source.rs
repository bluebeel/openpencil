//! Source codec fallback for lossless SVG image edge fields. No GL context.
use op_editor_core::svg_image_composition::{self as composition, EdgeSources, ImageSvgError};
pub fn install() {
    composition::install_source_decoder(decode);
}
fn decode(encoded: &[u8]) -> Result<EdgeSources, ImageSvgError> {
    let image =
        skia_safe::Image::from_encoded(skia_safe::Data::new_copy(encoded)).ok_or_else(|| {
            ImageSvgError::Codec {
                stage: "host source decoder",
                reason: "invalid or unavailable encoded media".into(),
            }
        })?;
    let (w, h) = (image.width().max(0) as u32, image.height().max(0) as u32);
    let bytes = (w as usize)
        .checked_mul(h as usize)
        .and_then(|v| v.checked_mul(8))
        .filter(|v| *v <= 384 * 1024 * 1024)
        .ok_or_else(|| ImageSvgError::Codec {
            stage: "source pixel budget",
            reason: "edge extraction exceeds the384MiB transient source budget".into(),
        })?;
    let mut pixels = vec![0u8; bytes];
    let info = skia_safe::ImageInfo::new(
        (w as i32, h as i32),
        skia_safe::ColorType::R16G16B16A16UNorm,
        skia_safe::AlphaType::Unpremul,
        skia_safe::ColorSpace::new_srgb(),
    );
    if !image.read_pixels(
        &info,
        &mut pixels,
        w as usize * 8,
        (0, 0),
        skia_safe::image::CachingHint::Disallow,
    ) {
        return Err(ImageSvgError::Codec {
            stage: "host source pixels",
            reason: "color-managed source readback failed".into(),
        });
    }
    let pixels: Vec<u16> = pixels
        .chunks_exact(2)
        .map(|b| u16::from_ne_bytes([b[0], b[1]]))
        .collect();
    composition::edges_from_rgba16(&pixels, w, h)
}
