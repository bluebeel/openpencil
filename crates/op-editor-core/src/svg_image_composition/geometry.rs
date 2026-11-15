use super::*;
use crate::image_field_geometry::{source_fit, ImageFieldError};
use jian_ops_schema::style::{ImageExtend, ImageFillMode, ImagePatternGeometry};
use kurbo::Affine;
/// The legacy normalized Figma UV map is distinct from explicit SVG geometry.
/// Preserve complete-turn f32 fitting instead of reducing authored angles.
pub fn legacy_geometry(
    paint: &ImageFillBody,
    bounds: [f64; 4],
    dimensions: Option<[u32; 2]>,
) -> Result<ImageFieldGeometry, ImageSvgError> {
    if let Some(g) = &paint.geometry {
        return Ok((**g).clone());
    }
    let w = bounds[2];
    let h = bounds[3];
    let mode = paint.mode.as_ref().unwrap_or(&ImageFillMode::Fill);
    let inverse = |m: Affine| {
        if m.determinant() == 0.0 {
            Err(ImageSvgError::InvalidGeometry {
                source: ImageFieldError::Singular,
            })
        } else {
            Ok(m.inverse())
        }
    };
    if *mode != ImageFillMode::Tile {
        if let Some(m) = &paint.transform {
            return Ok(ImageFieldGeometry {
                transform: inverse(Affine::new([m.m00, m.m10, m.m01, m.m11, m.m02, m.m12]))?
                    .as_coeffs(),
                extend_x: ImageExtend::Pad,
                extend_y: ImageExtend::Pad,
                ..Default::default()
            });
        }
        if *mode == ImageFillMode::Fill && paint.rotation.is_some_and(|r| r != 0.0) {
            let source = dimensions
                .ok_or(ImageSvgError::SourcePending)?
                .map(|v| v as f32);
            let [a, b, e, c, d, f] =
                source_fit(source, [w as f32, h as f32], paint.rotation.unwrap() as f32).map_err(
                    |_| ImageSvgError::InvalidGeometry {
                        source: ImageFieldError::Singular,
                    },
                )?;
            return Ok(ImageFieldGeometry {
                transform: inverse(Affine::new([
                    a as f64, c as f64, b as f64, d as f64, e as f64, f as f64,
                ]))?
                .as_coeffs(),
                extend_x: ImageExtend::Pad,
                extend_y: ImageExtend::Pad,
                ..Default::default()
            });
        }
    }
    if *mode == ImageFillMode::Stretch {
        return Ok(ImageFieldGeometry {
            extend_x: ImageExtend::Pad,
            extend_y: ImageExtend::Pad,
            ..Default::default()
        });
    }
    if dimensions.is_none() && paint.original_size.is_none() && *mode != ImageFillMode::Tile {
        return Ok(ImageFieldGeometry {
            viewport: Some(Box::new(jian_ops_schema::style::ImageViewportGeometry {
                size: [w, h],
                fit: if *mode == ImageFillMode::Fit {
                    jian_ops_schema::style::ImageViewportFit::Meet
                } else {
                    jian_ops_schema::style::ImageViewportFit::Slice
                },
                align: [0.5, 0.5],
                clip: true,
            })),
            ..Default::default()
        });
    }
    let source = paint
        .original_size
        .as_ref()
        .map(|s| [s.width, s.height])
        .or_else(|| dimensions.map(|d| d.map(f64::from)))
        .ok_or(ImageSvgError::SourcePending)?;
    if !source.iter().all(|n| n.is_finite() && *n > 0.0) {
        return Err(ImageSvgError::InvalidMetadata {
            stage: "source dimensions",
        });
    }
    if *mode == ImageFillMode::Tile {
        let scale = paint
            .tile_scale
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(1.0);
        // Match the existing Native/Web legacy Tile draw bound. Explicit SVG
        // pattern geometry does not acquire this legacy density clamp.
        let scale = scale
            .max(1.0 / source[0])
            .max(1.0 / source[1])
            .max(w.abs() / (128.0 * source[0]))
            .max(h.abs() / (128.0 * source[1]));
        let (tw, th) = (source[0] * scale / w, source[1] * scale / h);
        return Ok(ImageFieldGeometry {
            pattern: Some(Box::new(ImagePatternGeometry {
                transform: [tw, 0.0, 0.0, th, (1.0 - tw) * 0.5, (1.0 - th) * 0.5],
                ..Default::default()
            })),
            ..Default::default()
        });
    }
    let s = if *mode == ImageFillMode::Fit {
        (w / source[0]).min(h / source[1])
    } else {
        (w / source[0]).max(h / source[1])
    };
    let (sx, sy) = (source[0] * s / w, source[1] * s / h);
    let extend = if *mode == ImageFillMode::Fit {
        ImageExtend::Decal
    } else {
        ImageExtend::Pad
    };
    Ok(ImageFieldGeometry {
        transform: [sx, 0.0, 0.0, sy, (1.0 - sx) * 0.5, (1.0 - sy) * 0.5],
        extend_x: extend,
        extend_y: extend,
        ..Default::default()
    })
}
