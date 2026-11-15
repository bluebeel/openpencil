use super::*;
use image::{ImageDecoder, ImageEncoder};
use std::io::Cursor;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeSources {
    pub left: String,
    pub right: String,
    pub top: String,
    pub bottom: String,
    pub corners: [String; 4],
}
fn codec(stage: &'static str, error: impl std::fmt::Display) -> ImageSvgError {
    ImageSvgError::Codec {
        stage,
        reason: error.to_string(),
    }
}
/// Decode once; retain only eight tiny lossless source slices. No rendered node
/// pixels are read and no sampling grid or display scale is encoded here.
type SourceDecoder = fn(&[u8]) -> Result<EdgeSources, ImageSvgError>;
static SOURCE_DECODER: std::sync::OnceLock<SourceDecoder> = std::sync::OnceLock::new();
/// A native/Web host may provide its exact source codec for uncommon formats.
/// This decodes media pixels only, never a vector node or whole scene.
pub fn install_source_decoder(decoder: SourceDecoder) {
    let _ = SOURCE_DECODER.set(decoder);
}
struct Cached {
    key: Vec<u8>,
    edges: EdgeSources,
    bytes: usize,
}
#[derive(Default)]
struct Cache {
    entries: std::collections::VecDeque<Cached>,
    bytes: usize,
}
const CACHE_BYTES: usize = 8 * 1024 * 1024;
static CACHE: std::sync::OnceLock<std::sync::Mutex<Cache>> = std::sync::OnceLock::new();
fn edge_bytes(e: &EdgeSources) -> usize {
    e.left.capacity()
        + e.right.capacity()
        + e.top.capacity()
        + e.bottom.capacity()
        + e.corners.iter().map(String::capacity).sum::<usize>()
}
pub fn source_edges(encoded: &[u8]) -> Result<EdgeSources, ImageSvgError> {
    let cache = CACHE.get_or_init(Default::default);
    if let Ok(mut cache) = cache.lock() {
        if let Some(i) = cache
            .entries
            .iter()
            .position(|e| e.key.as_slice() == encoded)
        {
            let entry = cache.entries.remove(i).unwrap();
            let edges = entry.edges.clone();
            cache.entries.push_back(entry);
            return Ok(edges);
        }
    }
    let edges = portable_edges(encoded).or_else(|original| {
        SOURCE_DECODER
            .get()
            .map_or(Err(original), |decoder| decoder(encoded))
    })?;
    let bytes = encoded
        .len()
        .saturating_add(edge_bytes(&edges))
        .saturating_add(std::mem::size_of::<Cached>() + 128);
    if bytes <= CACHE_BYTES {
        if let Ok(mut cache) = cache.lock() {
            while cache.bytes.saturating_add(bytes) > CACHE_BYTES || cache.entries.len() >= 32 {
                let Some(old) = cache.entries.pop_front() else {
                    break;
                };
                cache.bytes = cache.bytes.saturating_sub(old.bytes);
            }
            cache.bytes = cache.bytes.saturating_add(bytes);
            cache.entries.push_back(Cached {
                key: encoded.to_vec(),
                edges: edges.clone(),
                bytes,
            });
        }
    }
    Ok(edges)
}

fn portable_edges(encoded: &[u8]) -> Result<EdgeSources, ImageSvgError> {
    let reader = image::ImageReader::new(Cursor::new(encoded))
        .with_guessed_format()
        .map_err(|e| codec("source format", e))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| codec("source decode", e))?;
    let (w, h) = decoder.dimensions();
    if (w as usize)
        .checked_mul(h as usize)
        .and_then(|v| v.checked_mul(8))
        .is_none_or(|v| v > 384 * 1024 * 1024)
    {
        return Err(codec(
            "source pixel budget",
            "edge extraction exceeds the384MiB transient source budget",
        ));
    }
    let profile = decoder
        .icc_profile()
        .map_err(|e| codec("source color profile", e))?;
    let orientation = decoder
        .orientation()
        .map_err(|e| codec("source orientation", e))?;
    let mut image =
        image::DynamicImage::from_decoder(decoder).map_err(|e| codec("source pixels", e))?;
    image.apply_orientation(orientation);
    let grayscale = matches!(
        image.color(),
        image::ColorType::L8
            | image::ColorType::La8
            | image::ColorType::L16
            | image::ColorType::La16
    );
    if profile
        .as_ref()
        .is_some_and(|p| p.get(16..20) == Some(b"CMYK"))
    {
        return Err(codec(
            "source color profile",
            "CMYK source requires the host's color-managed decoder",
        ));
    }
    let image = image.into_rgba16();
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 {
        return Err(ImageSvgError::SourcePending);
    }
    encode_edges(
        image.as_raw(),
        w,
        h,
        profile.as_deref(),
        grayscale,
        Some(encoded),
    )
}
pub fn edges_from_rgba16(pixels: &[u16], w: u32, h: u32) -> Result<EdgeSources, ImageSvgError> {
    encode_edges(pixels, w, h, None, false, None)
}
fn encode_edges(
    pixels: &[u16],
    w: u32,
    h: u32,
    profile: Option<&[u8]>,
    gray: bool,
    source: Option<&[u8]>,
) -> Result<EdgeSources, ImageSvgError> {
    if w == 0 || h == 0 || pixels.len() != w as usize * h as usize * 4 {
        return Err(ImageSvgError::SourcePending);
    }
    let encode = |x: u32, y: u32, width: u32, height: u32| -> Result<String, ImageSvgError> {
        let source_pixels = pixels;
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 8);
        for yy in y..y + height {
            for xx in x..x + width {
                let index = (yy as usize * w as usize + xx as usize) * 4;
                let p = &source_pixels[index..index + 4];
                if gray {
                    pixels.extend_from_slice(&p[0].to_ne_bytes());
                    pixels.extend_from_slice(&p[3].to_ne_bytes());
                } else {
                    for c in p {
                        pixels.extend_from_slice(&c.to_ne_bytes());
                    }
                }
            }
        }
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
        if let Some(profile) = &profile {
            encoder
                .set_icc_profile(profile.to_vec())
                .map_err(|e| codec("edge color profile", e))?;
        }
        encoder
            .write_image(
                &pixels,
                width,
                height,
                if gray {
                    image::ExtendedColorType::La16
                } else {
                    image::ExtendedColorType::Rgba16
                },
            )
            .map_err(|e| codec("edge PNG", e))?;
        Ok(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png_color_chunks(bytes, source))
        ))
    };
    Ok(EdgeSources {
        left: encode(0, 0, 1, h)?,
        right: encode(w - 1, 0, 1, h)?,
        top: encode(0, 0, w, 1)?,
        bottom: encode(0, h - 1, w, 1)?,
        corners: [
            encode(0, 0, 1, 1)?,
            encode(w - 1, 0, 1, 1)?,
            encode(w - 1, h - 1, 1, 1)?,
            encode(0, h - 1, 1, 1)?,
        ],
    })
}

// Carry valid PNG color chunks verbatim when only the source pixel rectangle
// changes. This includes grayscale profiles and gamma/chromaticity metadata.
fn png_color_chunks(encoded: Vec<u8>, source: Option<&[u8]>) -> Vec<u8> {
    let Some(source) = source.filter(|s| s.starts_with(b"\x89PNG\r\n\x1a\n")) else {
        return encoded;
    };
    let color = |kind: &[u8]| {
        matches!(
            kind,
            b"iCCP" | b"sRGB" | b"gAMA" | b"cHRM" | b"cICP" | b"mDCV" | b"cLLI"
        )
    };
    let mut chunks = Vec::new();
    let mut at = 8usize;
    while at + 12 <= source.len() {
        let len = u32::from_be_bytes(source[at..at + 4].try_into().unwrap()) as usize;
        let Some(end) = at
            .checked_add(len)
            .and_then(|v| v.checked_add(12))
            .filter(|v| *v <= source.len())
        else {
            break;
        };
        let kind = &source[at + 4..at + 8];
        if color(kind) {
            chunks.extend_from_slice(&source[at..end]);
        }
        if kind == b"IDAT" {
            break;
        }
        at = end;
    }
    if chunks.is_empty() {
        return encoded;
    }
    let mut output = encoded[..8].to_vec();
    at = 8;
    while at + 12 <= encoded.len() {
        let len = u32::from_be_bytes(encoded[at..at + 4].try_into().unwrap()) as usize;
        let Some(end) = at
            .checked_add(len)
            .and_then(|v| v.checked_add(12))
            .filter(|v| *v <= encoded.len())
        else {
            return encoded;
        };
        let kind = &encoded[at + 4..at + 8];
        if !color(kind) {
            output.extend_from_slice(&encoded[at..end]);
        }
        if kind == b"IHDR" {
            output.extend_from_slice(&chunks);
        }
        at = end;
    }
    output
}
