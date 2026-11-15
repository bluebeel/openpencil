//! Standard SVG image fields with source-identical media and typed restoration.
//! Pad strips are original source pixels, never rasterized node screenshots.
use base64::Engine;
use jian_ops_schema::style::{ImageFieldGeometry, ImageFillBody};
use serde::{Deserialize, Serialize};
mod geometry;
mod pixels;
mod visual;
pub use geometry::legacy_geometry;
pub use pixels::{edges_from_rgba16, install_source_decoder, source_edges, EdgeSources};
pub const METADATA_ATTRIBUTE: &str = "data-openpencil-image-v1";
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSvgError {
    InvalidBounds,
    InvalidIdentifier,
    InvalidGeometry {
        source: crate::image_field_geometry::ImageFieldError,
    },
    SourcePending,
    Codec {
        stage: &'static str,
        reason: String,
    },
    InvalidMetadata {
        stage: &'static str,
    },
}
impl std::fmt::Display for ImageSvgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBounds => f.write_str("image paint bounds must be finite and positive"),
            Self::InvalidIdentifier => f.write_str("invalid SVG image resource identifier"),
            Self::InvalidGeometry { source } => std::fmt::Display::fmt(source, f),
            Self::SourcePending => f.write_str("image source pixels are not loaded yet"),
            Self::Codec { stage, reason } => write!(f, "image {stage}: {reason}"),
            Self::InvalidMetadata { stage } => {
                write!(f, "invalid typed SVG image metadata at {stage}")
            }
        }
    }
}
impl std::error::Error for ImageSvgError {}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SvgImageMetadata {
    pub version: u32,
    pub bounds: [f64; 4],
    pub coverage: [f64; 4],
    /// URLs live once in real SVG image definitions, not twice in base64 JSON.
    pub paint: ImageFillBody,
    pub geometry: ImageFieldGeometry,
    pub dimensions: Option<[u32; 2]>,
    pub render_opacity: f64,
    pub display_thumbnail: bool,
}
pub fn encode_metadata(metadata: &SvgImageMetadata) -> Result<String, ImageSvgError> {
    serde_json::to_vec(metadata)
        .map(|v| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v))
        .map_err(|_| ImageSvgError::InvalidMetadata { stage: "encoding" })
}
pub fn decode_metadata(raw: &str) -> Result<SvgImageMetadata, ImageSvgError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| ImageSvgError::InvalidMetadata { stage: "base64" })?;
    let metadata: SvgImageMetadata = serde_json::from_slice(&bytes)
        .map_err(|_| ImageSvgError::InvalidMetadata { stage: "JSON" })?;
    if metadata.version != 1
        || !metadata.paint.url.is_empty()
        || metadata.paint.thumbnail_url.is_some()
    {
        return Err(ImageSvgError::InvalidMetadata {
            stage: "version or source references",
        });
    }
    Ok(metadata)
}
/// One coverage-sized paint server. Source patterns inside it preserve their
/// own affine cell, clipping and repeat domain instead of repeating node bounds.
pub fn definition(
    id: &str,
    metadata: &SvgImageMetadata,
    original: &str,
    thumbnail: Option<&str>,
    edges: Option<&EdgeSources>,
) -> Result<String, ImageSvgError> {
    for r in [metadata.bounds, metadata.coverage] {
        if !r.iter().all(|v| v.is_finite()) || r[2] <= 0.0 || r[3] <= 0.0 {
            return Err(ImageSvgError::InvalidBounds);
        }
    }
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
    {
        return Err(ImageSvgError::InvalidIdentifier);
    }
    if !metadata.render_opacity.is_finite() || !(0.0..=1.0).contains(&metadata.render_opacity) {
        return Err(ImageSvgError::InvalidMetadata { stage: "opacity" });
    }
    visual::definition(id, metadata, original, thumbnail, edges)
}
pub fn metadata(
    paint: &ImageFillBody,
    geometry: ImageFieldGeometry,
    bounds: [f64; 4],
    coverage: [f64; 4],
    dimensions: Option<[u32; 2]>,
    opacity: f64,
    display_thumbnail: bool,
) -> SvgImageMetadata {
    let mut paint = paint.clone();
    paint.url = "".into();
    paint.thumbnail_url = None;
    SvgImageMetadata {
        version: 1,
        bounds,
        coverage,
        paint,
        geometry,
        dimensions,
        render_opacity: opacity,
        display_thumbnail,
    }
}

/// Header/orientation only; no decoded frame allocation. SVG images use the
/// original source aspect even when their resident texture is downscaled.
pub fn source_dimensions(encoded: &[u8]) -> Option<[u32; 2]> {
    use image::ImageDecoder;
    let reader = image::ImageReader::new(std::io::Cursor::new(encoded))
        .with_guessed_format()
        .ok()?;
    if let Ok(mut decoder) = reader.into_decoder() {
        let (w, h) = decoder.dimensions();
        let orientation = decoder.orientation().ok().map(|o| o.to_exif()).unwrap_or(1);
        return Some(if matches!(orientation, 5..=8) {
            [h, w]
        } else {
            [w, h]
        });
    }
    op_util::encoded_image_dimensions(encoded).map(|(w, h)| [w, h])
}
