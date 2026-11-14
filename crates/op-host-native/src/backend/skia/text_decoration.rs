//! Font-metric decoration rectangles and descender gaps for one styled slice.
use super::super::{jian_color_to_color4f, shaped_text::ShapedRun, NativeBackend};
use op_editor_ui::{Rect, TextDecorationRequest};
use skia_safe::{Canvas, ClipOp, FontMetrics, Paint, RRect};

pub(super) fn draw(
    backend: &mut NativeBackend,
    canvas: &Canvas,
    request: &TextDecorationRequest<'_>,
) -> bool {
    if request.width <= 0.0 || (!request.underline && !request.strikethrough) {
        return true;
    }
    if !request.width.is_finite()
        || !request.origin.x.is_finite()
        || !request.origin.y.is_finite()
        || !request.font_size.is_finite()
        || request.font_size <= 0.0
        || request.text.chars().count() > 4096
    {
        return false;
    }
    let rectangles = jian_skia::with_font_lock(|| geometry(backend, request));
    let Some(rectangles) = rectangles else {
        return false;
    };
    for rect in rectangles {
        let modern = request.kerning.is_some() || !request.font_features.is_empty();
        if !modern || !sampled_rect(canvas, rect, request.color) {
            backend.fill_rect(canvas, rect, request.color);
        }
    }
    true
}

fn sampled_rect(canvas: &Canvas, rect: Rect, color: op_editor_ui::Color) -> bool {
    let paint = Paint::new(jian_color_to_color4f(color), None);
    if super::super::vector_text::draw_decoration_rect(
        canvas,
        super::super::to_sk_rect(rect),
        &paint,
    ) {
        return true;
    }
    let matrix = canvas.local_to_device_as_3x3();
    if !matrix.is_finite() || matrix.has_perspective() || matrix.invert().is_none() {
        return false;
    }
    let Some(clip) = canvas.device_clip_bounds() else {
        return true;
    };
    let local = super::super::to_sk_rect(rect);
    let (mut bounds, _) = matrix.map_rect(local);
    bounds.outset((1.0, 1.0));
    if !bounds.is_finite() || !bounds.intersect(skia_safe::Rect::from_irect(clip)) {
        return bounds.is_finite();
    }
    let saved = canvas.save();
    canvas.reset_matrix();
    canvas.clip_rect(bounds, ClipOp::Intersect, false);
    canvas.concat(&matrix);
    // Evaluate the same half-open rectangle as the outline sample lattice.
    // An aliased Skia rectangle can discard samples exactly on its top edge.
    super::super::sampled_clip::clip_rrect(canvas, &RRect::new_rect(local));
    let mut paint = Paint::new(jian_color_to_color4f(color), None);
    jian_skia::raster_sampling::configure_final_src_over(&mut paint);
    canvas.draw_paint(&paint);
    canvas.restore_to_count(saved);
    true
}

fn geometry(backend: &mut NativeBackend, request: &TextDecorationRequest<'_>) -> Option<Vec<Rect>> {
    let mut metrics = None;
    let mut ink = Vec::new();
    let color = jian_color_to_color4f(request.color);
    let mut collect = |font: &skia_safe::Font,
                       glyphs: &[skia_safe::GlyphId],
                       positions: &[skia_safe::Point],
                       relative_x: f32| {
        let (_, current) = font.metrics();
        let selected = *metrics.get_or_insert(current);
        if request.underline && request.skip_ink {
            if let Some((top, thickness)) = underline_metrics(selected) {
                let intervals =
                    font.get_intercepts(glyphs, positions, (top, top + thickness), None);
                for pair in intervals.chunks_exact(2) {
                    let (left, right) = (pair[0] + relative_x, pair[1] + relative_x);
                    if left.is_finite() && right.is_finite() && right > left {
                        ink.push((left, right));
                    }
                }
            }
        }
    };
    // Match the widget's whole-run versus per-character justification path.
    if (request.letter_spacing.abs() < f32::EPSILON
        || request.kerning == Some(true)
        || request.font_features.values().any(|v| *v != 0))
        && request.justify_extra <= 0.0
    {
        backend.shaped_text.visit_glyphs(
            &backend.font_resolver,
            &run_for(request, request.text, request.letter_spacing, color),
            &mut collect,
        );
    } else {
        let mut x = 0.0;
        for ch in request.text.chars() {
            let mut buffer = [0; 4];
            let single = run_for(request, ch.encode_utf8(&mut buffer), 0.0, color);
            backend.shaped_text.visit_glyphs(
                &backend.font_resolver,
                &single,
                |font, glyphs, positions, relative_x| {
                    collect(font, glyphs, positions, x + relative_x)
                },
            );
            x += backend.shaped_text.measure(&backend.font_resolver, &single)
                + request.letter_spacing;
            if ch == ' ' {
                x += request.justify_extra;
            }
        }
    }
    let metrics = metrics?;
    let baseline = request.origin.y.round();
    let mut rectangles = Vec::new();
    if request.underline {
        let (top, thickness) = underline_metrics(metrics)?;
        let segments = underline_segments(request.width, thickness, &mut ink);
        for (left, right) in segments {
            rectangles.push(Rect::xywh(
                request.origin.x + left,
                baseline + top,
                right - left,
                thickness,
            ));
        }
    }
    if request.strikethrough {
        let thickness = valid_positive(metrics.strikeout_thickness()?)?;
        let center = metrics.strikeout_position()?;
        if !center.is_finite() {
            return None;
        }
        rectangles.push(Rect::xywh(
            request.origin.x,
            baseline + center - thickness * 0.5,
            request.width,
            thickness,
        ));
    }
    Some(rectangles)
}

fn valid_positive(value: f32) -> Option<f32> {
    (value.is_finite() && value > 0.0).then_some(value)
}

fn underline_metrics(metrics: FontMetrics) -> Option<(f32, f32)> {
    let thickness = valid_positive(metrics.underline_thickness()?)?;
    let position = metrics.underline_position()?;
    if !position.is_finite() {
        return None;
    }
    // Glyph paint snaps the local baseline before the canvas CTM. Decoration
    // rectangles use the same baseline and the font's underline metric edge.
    Some((position + thickness * 0.5, thickness))
}

fn underline_segments(width: f32, halo: f32, ink: &mut [(f32, f32)]) -> Vec<(f32, f32)> {
    ink.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let mut result = Vec::new();
    let mut cursor = 0.0_f32;
    for &(left, right) in ink.iter() {
        let left = (left - halo).clamp(0.0, width);
        let right = (right + halo).clamp(0.0, width);
        if left > cursor {
            result.push((cursor, left));
        }
        cursor = cursor.max(right);
    }
    if cursor < width {
        result.push((cursor, width));
    }
    result
}

#[cfg(test)]
#[path = "text_decoration_tests.rs"]
mod tests;

fn run_for<'a>(
    request: &'a TextDecorationRequest<'_>,
    text: &'a str,
    spacing: f32,
    color: skia_safe::Color4f,
) -> ShapedRun<'a> {
    ShapedRun {
        text,
        family: request.font_family,
        font_size: request.font_size,
        weight: request.font_weight,
        italic: request.italic,
        kerning: request.kerning,
        letter_spacing: spacing,
        font_features: Some(request.font_features),
        line_height: request.line_height,
        color: skia_safe::Color::BLACK,
        source_color: Some(color),
    }
}
