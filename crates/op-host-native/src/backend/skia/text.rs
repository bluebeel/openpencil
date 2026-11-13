//! Text rendering for `NativeBackend` — typeface resolution caches,
//! per-script segmentation, weighted/styled measurement and
//! `draw_text`. Carved out of `skia.rs` (800-line cap) when italic
//! support landed; the typeface caches stay fields on the spine
//! struct, this sibling only houses the `impl` block.

use op_editor_core::text_script::needs_complex_shaping;
use op_editor_ui::{Point2D, TextBaselineRequest, TextLayout};

use super::shaped_text::ShapedRun;
use super::NativeBackend;

// Font parsing uses the render thread on Windows; these raster proofs run on
// the same macOS/Linux targets as the variable-font outline regressions.
#[cfg(all(test, not(target_os = "windows")))]
#[path = "text_raster_tests.rs"]
mod text_raster_tests;

pub(super) fn draw_text_runs(layout: &TextLayout) -> &[jian_core::render::TextRun] {
    layout.runs()
}

pub(super) fn quantize_advance(value: f32, font_size: f32) -> f32 {
    jian_skia::advance_grid::quantize_advance(value, font_size)
}

pub(super) fn uses_advance_grid(
    text: &str,
    kerning: Option<bool>,
    features: Option<&std::collections::BTreeMap<String, u32>>,
) -> bool {
    (kerning.is_some() || features.is_some_and(|features| !features.is_empty()))
        && jian_skia::advance_grid::supports_text(text)
}

impl NativeBackend {
    /// Resolve a typeface that covers `c`. Cached per codepoint —
    /// `FontMgr::match_family_style_character` is fast on first call
    /// but we still avoid the look-up on every chrome paint.
    /// Falls back to the cached CJK typeface, then the cached
    /// Roboto, so a worst-case missing-font system still renders
    /// something rather than dropping the glyph.
    pub(crate) fn typeface_for_char(
        &mut self,
        c: char,
        weight: u16,
    ) -> Option<skia_safe::Typeface> {
        self.font_resolver
            .typeface_for_char(None, c, weight, false)
            .map(|resolved| resolved.typeface)
    }

    /// Upright convenience wrapper — production paths go through the
    /// styled variant; the cache-behaviour tests exercise this one.
    #[cfg(test)]
    pub(crate) fn typeface_for_family_char(
        &mut self,
        c: char,
        family: &str,
        weight: u16,
    ) -> Option<skia_safe::Typeface> {
        self.font_resolver
            .typeface_for_char(Some(family), c, weight, false)
            .map(|resolved| resolved.typeface)
    }

    /// Measure the rendered horizontal advance of `text` at
    /// `font_size`. Uses the same per-script typeface dispatch as
    /// `draw_text` so the measurement matches what's painted.
    /// Falls back to a conservative heuristic when typefaces
    /// aren't available.
    pub fn measure_text(&mut self, text: &str, font_size: f32) -> f32 {
        self.measure_text_weighted(text, font_size, 400)
    }

    /// Weight-aware text measurement. Resolves the per-codepoint
    /// typeface against `FontStyle::new(Weight, ...)` so wrap-pass
    /// line breaks decided at, say, weight 700 use the same glyph
    /// advances `draw_text` will paint with at weight 700. Without
    /// this the wrap pass measured at 400 and paint at 700 — the
    /// rendered string could then overflow the wrap budget.
    pub fn measure_text_weighted(&mut self, text: &str, font_size: f32, weight: u16) -> f32 {
        self.measure_text_styled(text, font_size, weight, false)
    }

    /// Weight + slant aware measurement — styled text-run slices use
    /// it so a run resolved to a REAL italic face measures with that
    /// face's advances (a synthetic skew keeps upright advances, so
    /// both cases stay consistent with `draw_text`).
    pub fn measure_text_styled(
        &mut self,
        text: &str,
        font_size: f32,
        weight: u16,
        italic: bool,
    ) -> f32 {
        self.measure_text_family_styled(text, font_size, "", weight, italic)
    }

    /// Like [`Self::measure_text_styled`] but resolves the per-codepoint
    /// typeface against `family` — exactly as `draw_text` does for a run
    /// carrying that family. The family-blind `measure_text*` pass `""`
    /// (bundled Roboto); chrome inputs that DRAW in a named family (e.g.
    /// "Inter") must measure with that same family so their caret /
    /// selection geometry lines up with the painted glyphs.
    pub fn measure_text_family_styled(
        &mut self,
        text: &str,
        font_size: f32,
        family: &str,
        weight: u16,
        italic: bool,
    ) -> f32 {
        if needs_complex_shaping(text) {
            // Paint sends this run to the shaper, so measurement has to go
            // the same way. Measuring joined Arabic as a sum of isolated
            // advances would wrap lines and place carets against widths the
            // painter never uses.
            return self.shaped_text.measure(
                &self.font_resolver,
                &ShapedRun {
                    text,
                    family,
                    font_size,
                    weight,
                    italic,
                    kerning: None,
                    letter_spacing: 0.0,
                    font_features: None,
                    line_height: 0.0,
                    color: skia_safe::Color::BLACK,
                },
            );
        }
        self.font_resolver
            .measure_text(text, font_size, Some(family), weight, italic)
    }

    /// Explicit kerning follows the same Paragraph path as paint.
    #[allow(clippy::too_many_arguments)]
    pub fn measure_text_family_kerned(
        &mut self,
        text: &str,
        font_size: f32,
        family: &str,
        weight: u16,
        italic: bool,
        kerning: Option<bool>,
        letter_spacing: f32,
        font_features: &std::collections::BTreeMap<String, u32>,
    ) -> f32 {
        if kerning == Some(true) || font_features.values().any(|value| *value != 0) {
            self.shaped_text.measure(
                &self.font_resolver,
                &ShapedRun {
                    text,
                    family,
                    font_size,
                    weight,
                    italic,
                    kerning,
                    letter_spacing,
                    font_features: Some(font_features),
                    line_height: 0.0,
                    color: skia_safe::Color::BLACK,
                },
            )
        } else if uses_advance_grid(text, kerning, Some(font_features)) {
            self.measure_nominal_grid(text, font_size, family, weight, italic)
                + text.chars().count().saturating_sub(1) as f32 * letter_spacing
        } else {
            self.measure_text_family_styled(text, font_size, family, weight, italic)
                + text.chars().count().saturating_sub(1) as f32 * letter_spacing
        }
    }

    fn measure_nominal_grid(
        &mut self,
        text: &str,
        font_size: f32,
        family: &str,
        weight: u16,
        italic: bool,
    ) -> f32 {
        jian_skia::with_font_lock(|| {
            let segments = self
                .font_resolver
                .segment_text(text, Some(family), weight, italic);
            if segments.is_empty() {
                return self.font_resolver.measure_text(
                    text,
                    font_size,
                    Some(family),
                    weight,
                    italic,
                );
            }
            segments
                .into_iter()
                .map(|segment| {
                    let mut font = skia_safe::Font::new(&segment.typeface, font_size);
                    font.set_hinting(skia_safe::FontHinting::None);
                    font.set_subpixel(true);
                    let glyphs = font.str_to_glyphs_vec(&segment.text);
                    let mut widths = vec![0.0; glyphs.len()];
                    font.get_widths(&glyphs, &mut widths);
                    let factor = if segment.synthetic_bold {
                        jian_skia::SYNTHETIC_BOLD_WIDTH_FACTOR
                    } else {
                        1.0
                    };
                    widths
                        .into_iter()
                        .map(|width| quantize_advance(width * factor, font_size))
                        .sum::<f32>()
                })
                .sum()
        })
    }

    /// Family-aware width at the default weight/upright — the measurement
    /// backing `Painter::measure_text_family` for caret positioning.
    pub fn measure_text_family(&mut self, text: &str, font_size: f32, family: &str) -> f32 {
        self.measure_text_family_styled(text, font_size, family, 400, false)
    }

    /// Alphabetic ascent for the default typeface at `weight`.
    pub fn text_ascent(&mut self, font_size: f32, weight: u16) -> f32 {
        self.text_ascent_family(font_size, "", weight)
    }

    /// Alphabetic ascent for the same family resolver used by draw/measure.
    /// Skia reports ascent above the baseline as a negative value.
    pub fn text_ascent_family(&mut self, font_size: f32, family: &str, weight: u16) -> f32 {
        let fallback = font_size * 0.8;
        let Some(resolved) = self
            .font_resolver
            .typeface_for_char(Some(family), 'M', weight, false)
        else {
            return fallback;
        };
        let ascent = jian_skia::with_font_lock(|| {
            let font = skia_safe::Font::new(&resolved.typeface, font_size);
            let (_, metrics) = font.metrics();
            -metrics.ascent
        });
        if ascent.is_finite() && ascent > 0.0 {
            ascent
        } else {
            fallback
        }
    }

    /// First alphabetic baseline measured from the authored line-box top.
    /// Paragraph shapes the actual line, so fallback emoji/CJK faces and CSS
    /// half-leading participate in the same metric instead of sampling `M`.
    pub fn text_first_baseline(&mut self, request: &TextBaselineRequest<'_>) -> f32 {
        self.paragraph_baseline
            .first_line_baseline(
                &self.font_resolver,
                request.text,
                request.font_family,
                request.font_size,
                request.font_weight,
                request.italic,
                request.line_height,
            )
            .unwrap_or_else(|| {
                self.text_ascent_family(request.font_size, request.font_family, request.font_weight)
            })
    }

    /// Render every shaped run in the layout via cached typefaces +
    /// `Canvas::draw_str` (Step 4 perf fix — see comment on the
    /// `typeface` / `cjk_typeface` fields).
    ///
    /// Two fast paths + one fallback:
    ///   - run is ASCII-only → cached Roboto + draw_str
    ///   - run contains non-ASCII → cached system CJK typeface +
    ///     draw_str (PingFang / Noto CJK / etc.). PingFang covers
    ///     Latin too, so mixed CJK + Latin runs render correctly.
    ///   - the system has no CJK font (rare; Linux without Noto)
    ///     → fall back to jian-skia's textlayout path so glyphs
    ///     don't drop. Still slow on that branch, but functional.
    ///
    /// `layout.italic()` resolves a slanted face per codepoint; when
    /// the system serves no italic variant the glyphs are skewed
    /// synthetically (same philosophy as synthetic bold).
    #[tracing::instrument(skip(self, canvas, layout))]
    pub fn draw_text(&mut self, canvas: &skia_safe::Canvas, layout: &TextLayout, origin: Point2D) {
        // Serialize the whole text-run paint — typeface resolution plus glyph
        // rasterization via `draw_str`, both DirectWrite-backed on Windows —
        // with every other font user, so a concurrent design-worker layout
        // measure can't race skia's Windows font backend. Reentrant, so this
        // is a no-op re-lock when the caller already holds it.
        jian_skia::with_font_lock(|| {
            let italic = layout.italic();
            for run in draw_text_runs(layout) {
                let jc = run.color;
                if needs_complex_shaping(run.content.as_str())
                    || layout.kerning() == Some(true)
                    || layout.font_features().values().any(|value| *value != 0)
                {
                    // Arabic and friends need bidi reordering plus contextual
                    // joining, neither of which `draw_str` performs. Hand the
                    // whole run to the cached Paragraph shaper; segmentation
                    // would defeat shaping anyway, since joining is decided
                    // across the run, not per typeface span.
                    self.shaped_text.paint(
                        canvas,
                        &self.font_resolver,
                        &ShapedRun {
                            text: run.content.as_str(),
                            family: run.font_family.as_str(),
                            font_size: run.font_size,
                            weight: run.font_weight,
                            italic,
                            kerning: layout.kerning(),
                            letter_spacing: layout.letter_spacing(),
                            font_features: Some(layout.font_features()),
                            line_height: run.line_height,
                            color: skia_safe::Color::from_argb(jc.a(), jc.r(), jc.g(), jc.b()),
                        },
                        origin.x + run.origin.x,
                        origin.y + run.origin.y,
                    );
                    continue;
                }
                let segments = self.font_resolver.segment_text(
                    run.content.as_str(),
                    Some(&run.font_family),
                    run.font_weight,
                    italic,
                );
                if segments.is_empty() {
                    continue;
                }
                let mut paint = skia_safe::Paint::new(
                    skia_safe::Color4f::new(
                        f32::from(jc.r()) / 255.0,
                        f32::from(jc.g()) / 255.0,
                        f32::from(jc.b()) / 255.0,
                        f32::from(jc.a()) / 255.0,
                    ),
                    None,
                );
                paint.set_anti_alias(true);
                // Synthetic bold for typefaces the system serves at one
                // weight only (notably the bundled Roboto-Regular for
                // ASCII at weight ≥600). Stroke width scales with size
                // so 28pt headline gets the same visual weight relative
                // to its glyph as 13pt body text.
                let want_bold = run.font_weight >= 600;
                let modern = layout.kerning().is_some() || !layout.font_features().is_empty();
                let advance_grid = uses_advance_grid(
                    run.content.as_str(),
                    layout.kerning(),
                    Some(layout.font_features()),
                );
                let mut x = origin.x + run.origin.x;
                let y = origin.y + run.origin.y;
                for segment in segments {
                    let mut font = skia_safe::Font::new(&segment.typeface, run.font_size);
                    if segment.synthetic_italic {
                        font.set_skew_x(jian_skia::SYNTHETIC_ITALIC_SKEW);
                    }
                    // Keep advances identical to FontResolver's measurement;
                    // raster flags must not move following fallback segments.
                    let (mut advance, _) = font.measure_str(&segment.text, None);
                    // Document text follows scalable outlines at authored
                    // fractional positions. Platform grid fitting changes
                    // stem weight and rounds adjacent labels differently.
                    font.set_hinting(skia_safe::FontHinting::None);
                    font.set_subpixel(true);
                    if want_bold && segment.synthetic_bold {
                        paint.set_style(skia_safe::PaintStyle::StrokeAndFill);
                        paint.set_stroke_width(run.font_size * 0.06);
                    } else {
                        paint.set_style(skia_safe::PaintStyle::Fill);
                        paint.set_stroke_width(0.0);
                    }
                    if advance_grid {
                        let glyphs = font.str_to_glyphs_vec(&segment.text);
                        let mut widths = vec![0.0; glyphs.len()];
                        font.get_widths(&glyphs, &mut widths);
                        let factor = if segment.synthetic_bold {
                            jian_skia::SYNTHETIC_BOLD_WIDTH_FACTOR
                        } else {
                            1.0
                        };
                        let mut cursor = 0.0;
                        let positions: Vec<_> = widths
                            .into_iter()
                            .map(|width| {
                                let point = skia_safe::Point::new(cursor, 0.0);
                                cursor += quantize_advance(width * factor, run.font_size)
                                    + layout.letter_spacing();
                                point
                            })
                            .collect();
                        if !super::vector_text::draw(
                            canvas,
                            &font,
                            &glyphs,
                            &positions,
                            (x, y).into(),
                            &paint,
                            modern,
                        ) {
                            canvas.draw_glyphs_at(
                                &glyphs,
                                positions.as_slice(),
                                (x, y),
                                &font,
                                &paint,
                            );
                        }
                        advance = cursor;
                    } else {
                        let glyphs = font.str_to_glyphs_vec(&segment.text);
                        let mut positions = vec![skia_safe::Point::default(); glyphs.len()];
                        font.get_pos(&glyphs, &mut positions, None);
                        if !super::vector_text::draw(
                            canvas,
                            &font,
                            &glyphs,
                            &positions,
                            (x, y).into(),
                            &paint,
                            modern,
                        ) {
                            canvas.draw_str(&segment.text, (x, y), &font, &paint);
                        }
                        if segment.synthetic_bold {
                            advance *= jian_skia::SYNTHETIC_BOLD_WIDTH_FACTOR;
                        }
                    }
                    x += advance;
                }
            }
        });
    }
}
