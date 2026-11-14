//! Device-space sampled coverage for document glyph outlines.
//!
//! Shaping owns positions and advances. Only the local alphabetic baseline is
//! snapped here; fractional horizontal placement survives zoom and rotation.

use skia_safe::{
    AlphaType, BlendMode, Canvas, Color, Color4f, ColorType, Font, GlyphId, ImageInfo, Matrix,
    Paint, Path, PathBuilder, Point, Rect, SamplingOptions, TileMode,
};

#[path = "vector_text_curve.rs"]
mod curve;
#[path = "vector_text_reuse.rs"]
mod reuse;
#[path = "vector_text_stencil.rs"]
mod stencil;
#[path = "vector_text_units.rs"]
mod units;
pub(super) use units::MetadataCache;

pub(super) fn with_export_scope<R>(action: impl FnOnce() -> R) -> R {
    reuse::with_scope(action)
}

const MAX_EDGE: i32 = 8192;
const MAX_PIXELS: i64 = 16 * 1024 * 1024;
const MAX_GLYPHS: usize = 4096;

/// False leaves the canvas untouched so callers can retain their glyph-mask
/// fallback for legacy documents, color/bitmap faces and allocation limits.
pub(super) fn draw(
    canvas: &Canvas,
    font: &Font,
    glyphs: &[GlyphId],
    positions: &[Point],
    origin: Point,
    paint: &Paint,
    modern: bool,
    metadata: Option<&std::cell::RefCell<MetadataCache>>,
) -> bool {
    let point = jian_skia::raster_sampling::is_point_sampling();
    if (!modern && !point) || glyphs.len() != positions.len() || glyphs.len() > MAX_GLYPHS {
        if point {
            jian_skia::raster_sampling::mark_unsupported("invalid or oversized glyph outline run");
        }
        return false;
    }
    if font.is_embolden() || paint.mask_filter().is_some() || paint.image_filter().is_some() {
        if point {
            jian_skia::raster_sampling::mark_unsupported(
                "filtered or synthetic emboldened glyph run",
            );
        }
        return false;
    }
    let face = font.typeface();
    if [*b"COLR", *b"CBDT", *b"sbix", *b"SVG "]
        .iter()
        .any(|tag| face.get_table_size(u32::from_be_bytes(*tag)).is_some())
    {
        if point {
            jian_skia::raster_sampling::mark_unsupported("color or bitmap glyph outlines");
        }
        return false;
    }
    let path = reuse::outline(font, glyphs, positions, origin, modern, || {
        build_outline(font, glyphs, positions, origin, modern, metadata)
    });
    path.is_some_and(|path| draw_path(canvas, &path, paint))
}

fn build_outline(
    font: &Font,
    glyphs: &[GlyphId],
    positions: &[Point],
    origin: Point,
    modern: bool,
    metadata: Option<&std::cell::RefCell<MetadataCache>>,
) -> Option<Path> {
    let mut path = PathBuilder::new();
    for (&glyph, position) in glyphs.iter().zip(positions) {
        let x = origin.x + position.x;
        let y = origin.y.round() + position.y;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let outline = if modern {
            units::outline_with_metadata(font, glyph, metadata)
        } else {
            None
        };
        if let Some(outline) = outline.or_else(|| font.get_path(glyph)) {
            path.add_path_with_offset(&outline, (x, y), None);
        } else {
            let mut bounds = [Rect::default()];
            font.get_bounds(&[glyph], &mut bounds, None);
            if !bounds[0].is_empty() {
                if jian_skia::raster_sampling::is_point_sampling() {
                    jian_skia::raster_sampling::mark_unsupported("missing glyph outline");
                }
                return None;
            }
        }
    }
    Some(path.detach())
}

/// Decorations use the same fixed-point top-left rule as glyph triangles.
/// The same bounded fill coverage is shared with ordinary glyph outlines.
pub(super) fn draw_decoration_rect(canvas: &Canvas, rect: Rect, paint: &Paint) -> bool {
    if paint.style() != skia_safe::PaintStyle::Fill {
        return false;
    }
    let point = jian_skia::raster_sampling::is_point_sampling();
    let grid = if point {
        0
    } else if super::sampled_clip::is_four_sample_pass() {
        1
    } else {
        2
    };
    draw_coverage(canvas, &Path::rect(rect, None), paint, point, grid)
}

fn draw_coverage(canvas: &Canvas, path: &Path, paint: &Paint, point: bool, grid: u32) -> bool {
    let matrix = canvas.local_to_device_as_3x3();
    let Some(clip) = canvas.device_clip_bounds() else {
        return true;
    };
    let Some((shader, bounds)) = stencil::coverage(path, &matrix, clip, grid) else {
        return false;
    };
    let mut source = paint.clone();
    if point {
        jian_skia::raster_sampling::configure_final_src_over(&mut source);
    }
    let saved = canvas.save();
    canvas.clip_shader(shader, skia_safe::ClipOp::Intersect);
    canvas.draw_rect(bounds, &source);
    canvas.restore_to_count(saved);
    true
}

pub(super) fn draw_path(canvas: &Canvas, path: &Path, paint: &Paint) -> bool {
    if path.is_empty() {
        return true;
    }
    let point = jian_skia::raster_sampling::is_point_sampling();
    let grid = if point {
        0
    } else if super::sampled_clip::is_four_sample_pass() {
        1
    } else {
        2
    };
    // Polynomial cubic fills use the same bounded fixed4 edge rasterizer
    // after device-error-bounded subdivision. Do not send the resulting
    // polygon back through a second, different platform scan converter.
    let polygon = if point
        && paint.style() == skia_safe::PaintStyle::Fill
        && path
            .iter()
            .any(|record| record.verb() == skia_safe::PathVerb::Cubic)
        && !path
            .iter()
            .any(|record| record.verb() == skia_safe::PathVerb::Conic)
    {
        let matrix = canvas.local_to_device_as_3x3();
        reuse::flattened(path, &matrix, || curve::flatten(path, &matrix))
    } else {
        None
    };
    let path = polygon.as_ref().unwrap_or(path);
    // Whole-run context must not select a different edge rule for a flat
    // glyph. The same bounded fan rasterizer handles line-only contours.
    if paint.style() == skia_safe::PaintStyle::Fill
        && (path
            .iter()
            .any(|record| record.verb() == skia_safe::PathVerb::Quad)
            || path.iter().all(|record| {
                !matches!(
                    record.verb(),
                    skia_safe::PathVerb::Cubic | skia_safe::PathVerb::Conic
                )
            }))
        && draw_coverage(canvas, path, paint, point, grid)
    {
        return true;
    }
    if point {
        let mut point_paint = paint.clone();
        jian_skia::raster_sampling::configure_final_src_over(&mut point_paint);
        let matrix = canvas.local_to_device_as_3x3();
        let precise = reuse::flattened(path, &matrix, || curve::flatten(path, &matrix));
        canvas.draw_path(precise.as_ref().unwrap_or(path), &point_paint);
        return true;
    }
    let matrix = canvas.local_to_device_as_3x3();
    if !matrix.is_finite() || matrix.has_perspective() {
        return false;
    }
    let Some(inverse) = matrix.invert() else {
        return false;
    };
    // Include synthetic stroke geometry in coverage. Original color, shader,
    // alpha and backdrop blend still run exactly once after coverage resolves.
    let mut filled = PathBuilder::new();
    if !skia_safe::path_utils::fill_path_with_paint(path, paint, &mut filled, None, matrix) {
        return false;
    }
    let path = filled.detach().with_transform(&matrix);
    let matrix_identity = Matrix::new_identity();
    let path = reuse::flattened(&path, &matrix_identity, || {
        curve::flatten(&path, &matrix_identity)
    })
    .unwrap_or(path);
    let Some(clip) = canvas.device_clip_bounds() else {
        return true;
    };
    let mut bounds = *path.bounds();
    bounds.outset((1.0, 1.0));
    if !bounds.is_finite() || !bounds.intersect(Rect::from_irect(clip)) {
        return bounds.is_finite();
    }
    bounds = Rect::new(
        bounds.left.floor(),
        bounds.top.floor(),
        bounds.right.ceil(),
        bounds.bottom.ceil(),
    );
    let (width, height) = (bounds.width() as i32, bounds.height() as i32);
    if width <= 0
        || height <= 0
        || width > MAX_EDGE
        || height > MAX_EDGE
        || i64::from(width) * i64::from(height) > MAX_PIXELS
    {
        return false;
    }
    let grid = if super::sampled_clip::is_four_sample_pass() {
        1
    } else {
        2
    };
    // Rasterize the four camera offsets at 2x before resolving coverage.
    // Translating a 1x curve sixteen times changes Skia's edge quantization.
    // The actual mask remains subject to the original area and edge limits.
    let mut scale = mask_scale(width, height, grid);
    let make_mask = |scale| {
        let info = ImageInfo::new(
            (width * scale, height * scale),
            ColorType::RGBAF16,
            AlphaType::Premul,
            None,
        );
        skia_safe::surfaces::raster(&info, None, None)
    };
    let mask = make_mask(scale).or_else(|| {
        if scale == 1 {
            return None;
        }
        scale = 1;
        make_mask(scale)
    });
    let Some(mut mask) = mask else {
        return false;
    };
    let sample_grid = grid / scale;
    let mut coverage = Paint::default();
    coverage.set_color4f(
        Color4f::new(1.0, 1.0, 1.0, 1.0 / (4 * sample_grid * sample_grid) as f32),
        None,
    );
    coverage.set_blend_mode(BlendMode::Plus);
    coverage.set_anti_alias(false);
    let inner = mask.canvas();
    inner.clear(Color::TRANSPARENT);
    inner.scale((scale as f32, scale as f32));
    inner.translate((-bounds.left, -bounds.top));
    for y in 0..sample_grid {
        for x in 0..sample_grid {
            for (sx, sy) in [
                (0.375, 0.125),
                (0.875, 0.375),
                (0.125, 0.625),
                (0.625, 0.875),
            ] {
                let saved = inner.save();
                inner.translate((
                    (0.5 - (x as f32 + sx) / sample_grid as f32) / scale as f32,
                    (0.5 - (y as f32 + sy) / sample_grid as f32) / scale as f32,
                ));
                inner.draw_path(&path, &coverage);
                inner.restore_to_count(saved);
            }
        }
    }
    let image_to_device = Matrix::concat(
        &Matrix::translate((bounds.left, bounds.top)),
        &Matrix::scale((1.0 / scale as f32, 1.0 / scale as f32)),
    );
    let image_to_local = Matrix::concat(&inverse, &image_to_device);
    let sampling = if scale == 2 {
        SamplingOptions::new(skia_safe::FilterMode::Linear, skia_safe::MipmapMode::None)
    } else {
        SamplingOptions::default()
    };
    let Some(shader) = mask.image_snapshot().to_shader(
        (TileMode::Decal, TileMode::Decal),
        sampling,
        &image_to_local,
    ) else {
        return false;
    };
    let saved = canvas.save();
    // A shader clip has no finite raster bounds. Restrict the final paint to
    // the device mask before restoring the text transform; otherwise every
    // short text run evaluates its coverage across the entire export surface.
    canvas.reset_matrix();
    canvas.clip_rect(bounds, skia_safe::ClipOp::Intersect, false);
    canvas.concat(&matrix);
    canvas.clip_shader(shader, skia_safe::ClipOp::Intersect);
    // Keep the original color, opacity, clipping and backdrop blend once.
    // Global per-pass RGBA8 compositing requires correlated scene samples.
    canvas.draw_paint(paint);
    canvas.restore_to_count(saved);
    true
}

fn mask_scale(width: i32, height: i32, grid: i32) -> i32 {
    if grid == 2
        && width <= MAX_EDGE / 2
        && height <= MAX_EDGE / 2
        && i64::from(width) * i64::from(height) <= MAX_PIXELS / 4
    {
        2
    } else {
        1
    }
}

#[cfg(all(test, not(target_os = "windows")))]
#[path = "vector_text_tests.rs"]
mod tests;
