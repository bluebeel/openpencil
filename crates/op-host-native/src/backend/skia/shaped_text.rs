//! Shaped paint + measure for complex scripts.
//!
//! The surrounding fast path (`text.rs`) maps codepoints straight to glyphs
//! via `Canvas::draw_str`, which does no bidi reordering and no contextual
//! glyph selection — correct and cheap for Latin and CJK, wrong for Arabic.
//! Runs that `op_editor_core::text_script::needs_complex_shaping` flags come
//! here instead and go through Skia's Paragraph shaper (ICU bidi +
//! HarfBuzz).
//!
//! The reason jian's `draw_text_paragraph` was bypassed natively is that it
//! builds a fresh `FontCollection` on every call (~605 ms chrome frames).
//! This type keeps the collection cached with the same generation-guard
//! `jian_skia::ParagraphBaseline` uses, so the shaper is affordable on the
//! paint path.

use std::rc::Rc;

#[path = "shaped_text_features.rs"]
mod features;
#[path = "shaped_text_cache.rs"]
mod paragraph_cache;

use jian_skia::FontResolver;
use op_editor_core::text_script::{base_direction, BaseDirection};
use skia_safe::{
    font_style::{Slant, Weight, Width},
    textlayout::{FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, TextStyle},
    Canvas, FontStyle,
};

/// The default family jian seeds its own collections with, so a shaped run
/// falls back to the same face an unshaped one would.
const DEFAULT_FAMILY: &str = "Roboto";

/// Effectively unbounded wrap budget. Every run reaching paint is already a
/// single line (wrapping happens upstream in `wrap_text`), so the paragraph
/// must not break; `max_intrinsic_width` then reports the natural width.
const NATURAL_LAYOUT_BUDGET: f32 = 1.0e6;

// Font parsing uses DirectWrite on Windows and must stay on its render thread.
#[cfg(all(test, not(target_os = "windows")))]
#[path = "shaped_advance_tests.rs"]
mod shaped_advance_tests;
#[cfg(all(test, not(target_os = "windows")))]
#[path = "variable_font_tests.rs"]
mod variable_font_tests;

/// One styled run to shape. Grouped into a struct so paint and measure take
/// the identical inputs — if they ever drifted, wrapped text would be laid
/// out against one width and painted at another.
pub(crate) struct ShapedRun<'a> {
    pub text: &'a str,
    pub family: &'a str,
    pub font_size: f32,
    pub weight: u16,
    pub italic: bool,
    pub kerning: Option<bool>,
    pub letter_spacing: f32,
    pub font_features: Option<&'a std::collections::BTreeMap<String, u32>>,
    pub line_height: f32,
    pub color: skia_safe::Color,
    pub source_color: Option<skia_safe::Color4f>,
}

pub(crate) struct ShapedText {
    collection: Rc<FontCollection>,
    built_generation: u64,
    paragraphs: paragraph_cache::Cache,
    pub(super) metadata: std::cell::RefCell<super::vector_text::MetadataCache>,
}

impl ShapedText {
    pub(crate) fn new(font_resolver: &FontResolver) -> Self {
        jian_skia::with_font_lock(|| Self {
            // Read the generation before building: a concurrent font
            // registration can at worst leave this stale-low, costing one
            // harmless rebuild.
            built_generation: jian_skia::font_generation(),
            paragraphs: paragraph_cache::Cache::default(),
            metadata: std::cell::RefCell::new(super::vector_text::MetadataCache::default()),
            collection: Rc::new(build_collection(font_resolver)),
        })
    }

    fn collection(&mut self, font_resolver: &FontResolver) -> Rc<FontCollection> {
        let generation = jian_skia::font_generation();
        if generation != self.built_generation {
            self.paragraphs.clear();
            self.collection = Rc::new(build_collection(font_resolver));
            self.built_generation = generation;
        }
        Rc::clone(&self.collection)
    }

    /// Natural, unwrapped advance width of the shaped run.
    pub(crate) fn measure(&mut self, font_resolver: &FontResolver, run: &ShapedRun<'_>) -> f32 {
        jian_skia::with_font_lock(|| {
            self.with_paragraph(font_resolver, run, false, |paragraph, _| {
                let mut width = paragraph.max_intrinsic_width();
                if super::text::uses_advance_grid(run.text, run.kerning, run.font_features) {
                    let mut corrections = std::collections::BTreeMap::<usize, f32>::new();
                    let mut clusters = std::collections::BTreeMap::<usize, TrackingClusters>::new();
                    paragraph.visit(|line, info| {
                        if let Some(info) = info {
                            let tracking = clusters.entry(line).or_default();
                            for &cluster in info.utf8_starts().iter().take(info.glyphs().len()) {
                                tracking.ordinal(cluster);
                            }
                            if let Some(adjusted) = jian_skia::advance_grid::adjust_glyph_run(
                                info.font(),
                                info.glyphs(),
                                info.positions(),
                                info.utf8_starts(),
                                info.advance_x(),
                            ) {
                                *corrections.entry(line).or_default() +=
                                    adjusted.advance_correction;
                            }
                        }
                    });
                    if run.text.contains('\n') {
                        width = paragraph
                            .get_line_metrics()
                            .iter()
                            .enumerate()
                            .map(|(line, metrics)| {
                                jian_skia::advance_grid::tracked_position(
                                    jian_skia::advance_grid::quantize_advance(
                                        metrics.width as f32
                                            + corrections.get(&line).copied().unwrap_or(0.0),
                                        run.font_size,
                                    ),
                                    clusters.get(&line).map_or(0, |c| c.count.saturating_sub(1)),
                                    run.letter_spacing,
                                )
                            })
                            .fold(0.0, f32::max);
                        return width;
                    } else {
                        width = jian_skia::advance_grid::quantize_advance(
                            width + corrections.get(&0).copied().unwrap_or(0.0),
                            run.font_size,
                        );
                        return jian_skia::advance_grid::tracked_position(
                            width,
                            clusters
                                .get(&0)
                                .map_or_else(
                                    || TrackingClusters::default().including_trailing(run.text),
                                    |c| c.including_trailing(run.text),
                                )
                                .saturating_sub(1),
                            run.letter_spacing,
                        );
                    }
                }
                width + run.text.chars().count().saturating_sub(1) as f32 * run.letter_spacing
            })
        })
    }

    /// Visit the same unwrapped glyph geometry used by paint, for ink gaps.
    pub(super) fn visit_glyphs(
        &mut self,
        font_resolver: &FontResolver,
        run: &ShapedRun<'_>,
        action: impl FnMut(&skia_safe::Font, &[skia_safe::GlyphId], &[skia_safe::Point], f32),
    ) {
        self.with_paragraph(font_resolver, run, true, |paragraph, _| {
            visit_positioned(paragraph, run, action);
        });
    }

    /// Paint the run with `baseline_y` on its alphabetic baseline, matching
    /// the `draw_str` convention the unshaped path uses, so shaped and
    /// unshaped text sit on one line.
    pub(crate) fn paint(
        &mut self,
        canvas: &Canvas,
        font_resolver: &FontResolver,
        run: &ShapedRun<'_>,
        x: f32,
        baseline_y: f32,
    ) {
        jian_skia::with_font_lock(|| {
            self.with_paragraph(font_resolver, run, true, |paragraph, metadata| {
                // Two passes. A right-to-left paragraph anchors against the RIGHT
                // edge of its layout box, so laying out once against the probe
                // budget would fling Arabic a million pixels off-screen. Measure
                // the natural width first, then re-lay out into a box of exactly
                // that width so the run occupies the same span at `x` either way.
                //
                // `ceil` rather than the raw width: a box even a float-epsilon
                // narrower than the text makes the shaper wrap, turning one line
                // into two. Rounding up trades a sub-pixel anchor shift for
                // never breaking a line that should not break.
                if jian_skia::raster_sampling::is_point_sampling()
                    || (run.kerning == Some(true)
                        || run
                            .font_features
                            .is_some_and(|f| f.values().any(|value| *value != 0)))
                        && !op_editor_core::text_script::needs_complex_shaping(run.text)
                {
                    // HarfBuzz supplies pair positioning; the direct native font
                    // path supplies the same raster treatment as unkerned text.
                    // Paragraph's visitor origin contains a rounded line baseline,
                    // so retain the caller's authored baseline for this one line.
                    let mut paint = skia_safe::Paint::default();
                    paint.set_color(run.color);
                    if let Some(color) = run.source_color {
                        paint.set_color4f(color, None);
                    }
                    jian_skia::raster_sampling::configure_geometry(&mut paint);
                    let modern = run.kerning.is_some()
                        || run
                            .font_features
                            .is_some_and(|features| !features.is_empty());
                    visit_positioned(paragraph, run, |font, glyphs, positions, relative_x| {
                        let origin = (x + relative_x, baseline_y);
                        if !super::vector_text::draw(
                            canvas,
                            font,
                            glyphs,
                            positions,
                            origin.into(),
                            &paint,
                            modern,
                            Some(metadata),
                        ) {
                            canvas.draw_glyphs_at(glyphs, positions, origin, font, &paint);
                        }
                    });
                    return;
                }
                // `Paragraph::paint` places the line-box top; callers hand us a
                // baseline.
                let top = baseline_y - paragraph.alphabetic_baseline();
                paragraph.paint(canvas, (x, top));
            });
        });
    }

    fn build(&mut self, font_resolver: &FontResolver, run: &ShapedRun<'_>) -> Paragraph {
        let collection = self.collection(font_resolver);
        let mut paragraph_style = ParagraphStyle::new();
        // Without this the shaper bidi-reorders correctly but anchors the
        // paragraph left-to-right, so a trailing neutral (the "." in an
        // abbreviation like "ك.م") lands on the wrong end.
        paragraph_style.set_text_direction(match base_direction(run.text) {
            BaseDirection::Rtl => skia_safe::textlayout::TextDirection::RTL,
            BaseDirection::Ltr => skia_safe::textlayout::TextDirection::LTR,
        });

        let explicit_shape = run.kerning == Some(true)
            || run
                .font_features
                .is_some_and(|f| f.values().any(|value| *value != 0));
        if explicit_shape {
            paragraph_style.turn_hinting_off();
        }
        let mut text_style = TextStyle::new();
        text_style.set_font_size(run.font_size);
        features::apply(&mut text_style, run.font_features, run.kerning);
        if explicit_shape {
            text_style.set_font_hinting(skia_safe::FontHinting::None);
        }
        text_style.set_color(run.color);
        let families = font_resolver.font_families_for_shaping_shared(if run.family.is_empty() {
            None
        } else {
            Some(run.family)
        });
        if !families.is_empty() {
            text_style.set_font_families(families.as_ref());
        }
        text_style.set_font_style(FontStyle::new(
            Weight::from(i32::from(run.weight)),
            Width::NORMAL,
            if run.italic {
                Slant::Italic
            } else {
                Slant::Upright
            },
        ));
        jian_skia::font_variations::set_paragraph_weight(&mut text_style, run.weight);
        if run.line_height > 0.0 {
            text_style.set_height(run.line_height);
            text_style.set_height_override(true);
            text_style.set_half_leading(true);
        }

        let mut builder = ParagraphBuilder::new(&paragraph_style, (*collection).clone());
        builder.push_style(&text_style);
        builder.add_text(run.text);
        builder.pop();
        builder.build()
    }
}

fn build_collection(font_resolver: &FontResolver) -> FontCollection {
    let mut collection = FontCollection::new();
    collection.set_default_font_manager(font_resolver.ordered_font_manager(), Some(DEFAULT_FAMILY));
    if let Some(provider) = jian_skia::bundled_fonts::asset_provider() {
        collection.set_asset_font_manager(Some(provider.into()));
    }
    collection
}

fn visit_positioned(
    paragraph: &mut Paragraph,
    run: &ShapedRun<'_>,
    mut action: impl FnMut(&skia_safe::Font, &[skia_safe::GlyphId], &[skia_safe::Point], f32),
) {
    let use_grid = super::text::uses_advance_grid(run.text, run.kerning, run.font_features);
    // Only legacy nonzero tracking looks up UTF-8 character ordinals. Grid
    // tracking uses its shaped cluster map, and zero tracking uses neither.
    let character_offsets = if run.letter_spacing != 0.0 && !use_grid {
        run.text
            .char_indices()
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut advance_correction = 0.0;
    let mut tracking = TrackingClusters::default();
    let mut current_line = None;
    paragraph.visit(|line, info| {
        if current_line != Some(line) {
            current_line = Some(line);
            advance_correction = 0.0;
            tracking = TrackingClusters::default();
        }
        let Some(info) = info else { return };
        let source = info.font();
        let mut font = skia_safe::Font::new(source.typeface(), source.size());
        font.set_skew_x(source.skew_x());
        font.set_embolden(source.is_embolden());
        font.set_hinting(skia_safe::FontHinting::None);
        font.set_subpixel(true);
        let adjusted = if use_grid {
            jian_skia::advance_grid::adjust_glyph_run(
                source,
                info.glyphs(),
                info.positions(),
                info.utf8_starts(),
                info.advance_x(),
            )
        } else {
            None
        };
        let base_positions = adjusted
            .as_ref()
            .map_or(info.positions(), |run| run.positions.as_slice());
        let origin_correction = advance_correction;
        if let Some(run) = &adjusted {
            advance_correction += run.advance_correction;
        }
        let positions;
        let glyph_positions = if run.letter_spacing != 0.0 {
            positions = base_positions
                .iter()
                .zip(info.utf8_starts())
                .map(|(position, offset)| {
                    let character = if use_grid {
                        tracking.ordinal(*offset)
                    } else {
                        character_offsets.partition_point(|start| *start < *offset as usize)
                    };
                    skia_safe::Point::new(
                        if use_grid {
                            jian_skia::advance_grid::tracked_position(
                                position.x,
                                character,
                                run.letter_spacing,
                            )
                        } else {
                            position.x + character as f32 * run.letter_spacing
                        },
                        position.y,
                    )
                })
                .collect::<Vec<_>>();
            positions.as_slice()
        } else {
            base_positions
        };
        action(
            &font,
            info.glyphs(),
            glyph_positions,
            info.origin().x + origin_correction,
        );
    });
}

/// Modern authored tracking is applied between shaped clusters, so an fi/ffi
/// ligature receives one spacing step. Consecutive glyphs of one cluster keep
/// their original mark offsets; unsupported/legacy text retains its old path.
#[derive(Default)]
struct TrackingClusters {
    last: Option<u32>,
    count: usize,
}
impl TrackingClusters {
    fn including_trailing(&self, text: &str) -> usize {
        // Paragraph's paint visitor can omit trailing whitespace while its
        // intrinsic advance still includes it. Those source clusters also
        // separate a following styled slice and must retain their tracking.
        // Count only the unvisited suffix: visible ligatures keep one cluster,
        // and whitespace already reported by a different backend stays single.
        self.count
            + text
                .char_indices()
                .rev()
                .take_while(|(_, ch)| ch.is_whitespace())
                .filter(|(offset, _)| self.last.is_none_or(|last| *offset > last as usize))
                .count()
    }

    fn ordinal(&mut self, cluster: u32) -> usize {
        if self.last != Some(cluster) {
            self.last = Some(cluster);
            self.count += 1;
        }
        self.count.saturating_sub(1)
    }
}
