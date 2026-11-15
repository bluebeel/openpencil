//! `EditorState` → [`LayoutScene`] builder.
//!
//! Produces the paint-only, layout-resolved scene walked by `CanvasViewport`.
//!
//! The flex layout pass is NOT re-implemented here. `EditorState.doc`
//! is a `PenDocument`; [`pen_document_to_payload`] runs each page-root
//! through jian-core's taffy `LayoutEngine` + `jian_skia::SkiaMeasure`
//! (see `adapter.rs`) and bakes the resolved absolute AABBs — plus
//! every paint field — into a layout-resolved [`DocPayload`]. This
//! builder reuses that resolved payload and re-shapes its `NodePayload`
//! tree into [`SceneNode`]s, dropping all editor state (selection /
//! chat / history / ui) and resolving variable `$ref` fills against
//! the editor's variables + active theme.
//!
//! So the resolved geometry a `LayoutScene` carries is bit-identical
//! to what `pen_document_to_payload` bakes — there is one layout pass
//! and one set of resolved rects. The builder no longer routes through
//! the shell-core `Document` model: `DocPayload` already carries the
//! resolved geometry + paint fields, and `apply_payload`'s only
//! transforms on them are lossless format conversions (colour array →
//! struct, kind / fill-type string → enum).
use crate::payload::{
    DocPayload, GradientPayload, GradientStopPayload, NodePayload, ShaderPayload,
};
use jian_scene::layout_scene::NodeKind;
use jian_scene::layout_scene::{
    stable_image_source_id, DropShadow, Effect, LayoutScene, SceneFillLayer, SceneFillType,
    SceneGradient, SceneGradientStop, SceneImageFit, SceneNode, ScenePage, SceneShader,
    SceneShaderUniform, SceneTextAlign, SceneTextRun, SceneTextVerticalAlign, SceneVideo,
};
use op_editor_core::render_backend::{Color, ImageBlendMode};
use op_editor_core::scene_vars::VariableTable;
use std::sync::Arc;

// Keep the legacy test module's historical local name; implementation lives in `editor_scene`.
#[cfg(test)]
use crate::editor_scene::editor_state_to_layout_scene;

#[path = "layout_scene_owned.rs"]
mod owned;
pub(crate) use owned::{node_payload_to_scene, payload_into_scene};

mod stroke;
mod stroke_paint;
use stroke::{is_status_bar_shell_stroke, is_unpainted_widget_stroke, scene_stroke};
use stroke_paint::stroke_paint_to_scene;
mod widget;
use widget::widget_payload_to_scene;

/// Build a [`LayoutScene`] from a bare [`PenDocument`], theme, and active page index.
///
/// Runs the SAME render-time ref + token resolution and flex layout as
/// [`editor_state_to_layout_scene`]; Canvas Preview uses this to render
/// its prepared + promoted runtime document. Tokens resolve against
/// `active_theme`; with no transient editor fill/stroke-ref caches the
/// var table comes straight from the document's `variables` / `themes`
/// — sufficient because the resolved doc already carries concrete
/// colours, so `node_payload_to_scene`'s `$ref` lookups all miss and
/// fall back to the node's own (now literal) fill.
///
/// [`EditorState`]: op_editor_core::EditorState
pub fn pen_document_to_layout_scene(
    doc: &jian_ops_schema::PenDocument,
    active_theme: &std::collections::BTreeMap<String, String>,
    active_page_index: usize,
) -> LayoutScene {
    pen_document_to_layout_scene_with_geometry_mode(doc, active_theme, active_page_index, false)
}

/// Build a [`LayoutScene`] from a bare [`PenDocument`] while selecting how
/// node geometry is resolved.
///
/// `preserve_authored_geometry` is intended for Preserve-mode Figma imports:
/// those documents already carry numeric parent-local positions and sizes, so
/// running flex layout again would both waste work and move overlapping
/// children away from their authored coordinates. Passing `false` retains the
/// historical behavior of [`pen_document_to_layout_scene`].
pub fn pen_document_to_layout_scene_with_geometry_mode(
    doc: &jian_ops_schema::PenDocument,
    active_theme: &std::collections::BTreeMap<String, String>,
    active_page_index: usize,
    preserve_authored_geometry: bool,
) -> LayoutScene {
    let mut prepared = std::borrow::Cow::Borrowed(doc);
    if op_editor_core::ref_resolve::document_has_refs(&prepared) {
        prepared = std::borrow::Cow::Owned(op_editor_core::ref_resolve::resolve_refs_for_canvas(
            &prepared,
        ));
    }
    if op_editor_core::variables_resolve::document_has_tokens(&prepared) {
        prepared = std::borrow::Cow::Owned(
            op_editor_core::variables_resolve::resolve_document_for_canvas(&prepared, active_theme),
        );
    }
    let payload: DocPayload = if preserve_authored_geometry {
        crate::adapter::pen_document_to_payload_preserving_geometry(&prepared).payload
    } else {
        crate::adapter::pen_document_to_payload(&prepared).payload
    };
    let mut var_table = crate::adapter::build_var_table(&prepared);
    var_table.active_theme = active_theme.clone();
    drop(prepared);
    payload_into_scene(payload, &var_table, active_page_index)
}

/// Build the Canvas Preview [`LayoutScene`]: paint tree from the
/// PROMOTED document (so legacy `role=input` frames render as live
/// widgets), geometry from the UNPROMOTED document laid out exactly as
/// the design canvas lays it out — honoring
/// `preserve_authored_geometry` for Figma Preserve imports. Node
/// positions in the returned scene therefore match the design canvas
/// by construction; only the paint representation differs.
///
/// Both documents must already be ref/token-resolved (the preview
/// session prepares them before promotion), so no resolution passes
/// run here; the var table still folds the document's variables +
/// `active_theme` for any `$ref` fill lookups.
pub fn pen_document_to_layout_scene_for_preview(
    paint_doc: &jian_ops_schema::PenDocument,
    layout_doc: &jian_ops_schema::PenDocument,
    preserve_authored_geometry: bool,
    active_theme: &std::collections::BTreeMap<String, String>,
    active_page_index: usize,
) -> LayoutScene {
    pen_document_to_layout_scene_for_preview_with_measure(
        paint_doc,
        layout_doc,
        preserve_authored_geometry,
        active_theme,
        active_page_index,
        crate::adapter::pages::layout_measure_backend(),
    )
}

/// Build preview paint geometry with the exact backend used for runtime hits.
pub fn pen_document_to_layout_scene_for_preview_with_measure(
    paint_doc: &jian_ops_schema::PenDocument,
    layout_doc: &jian_ops_schema::PenDocument,
    preserve_authored_geometry: bool,
    active_theme: &std::collections::BTreeMap<String, String>,
    active_page_index: usize,
    measure: std::rc::Rc<dyn jian_core::layout::measure::MeasureBackend>,
) -> LayoutScene {
    let payload: DocPayload = crate::adapter::pen_documents_to_payload_for_preview(
        paint_doc,
        layout_doc,
        preserve_authored_geometry,
        measure,
    )
    .payload;
    let mut var_table = crate::adapter::build_var_table(paint_doc);
    var_table.active_theme = active_theme.clone();
    payload_into_scene(payload, &var_table, active_page_index)
}

/// Map payload text runs onto scene runs: each segment's `text` length
/// becomes a byte range into the node's flattened string (the payload
/// flattens segments in order, so ranges are cumulative). Per-run fill
/// colours get the node's direct paint opacity folded into their alpha — the
/// same treatment as the node-level fill. Isolated group alpha is applied by
/// the painter after all runs and descendants have assembled.
fn text_runs_to_scene(
    runs: &[crate::payload::TextRunPayload],
    paint_opacity: f32,
) -> Vec<SceneTextRun> {
    let mut start = 0usize;
    runs.iter()
        .map(|run| {
            let end = start + run.text.len();
            let scene = SceneTextRun {
                start,
                end,
                fill_disabled: run.fill_disabled,
                font_size: run.font_size,
                font_weight: run.font_weight,
                fill: run
                    .fill
                    .map(array_to_color)
                    .map(|c| mul_alpha(c, paint_opacity)),
                italic: run.italic,
                underline: run.underline,
                strikethrough: run.strikethrough,
                text_decoration_skip_ink: run.text_decoration_skip_ink,
            };
            start = end;
            scene
        })
        .collect()
}

/// Fold node opacity into an effect's colour. A drop shadow is part
/// of the node's own paint, so node opacity dims it alongside the
/// fill (a 30%-opacity node casts a 30%-strength shadow).
fn scale_effect_opacity(e: Effect, k: f32) -> Effect {
    match e {
        Effect::DropShadow(s) => Effect::DropShadow(DropShadow {
            color: mul_alpha(s.color, k),
            ..s
        }),
        // Blur (layer or backdrop) has no colour to fold node opacity into.
        Effect::Blur(b) => Effect::Blur(b),
        Effect::BackgroundBlur { radius } => Effect::BackgroundBlur { radius },
    }
}

/// Multiply a colour's alpha by `k` (node-opacity folding).
fn mul_alpha(c: Color, k: f32) -> Color {
    Color {
        a: (c.a * k).clamp(0.0, 1.0),
        ..c
    }
}

/// Fold node opacity into a gradient by scaling its alpha multiplier
/// only. The backend folds `opacity` into every stop colour when it
/// builds the shader (`gradient_color_arrays`), so scaling the stops
/// here too would dim the gradient twice. Leave stops at their
/// authored alpha; the single `opacity` multiplier carries node
/// opacity.
fn scale_gradient_opacity(g: SceneGradient, k: f32) -> SceneGradient {
    match g {
        SceneGradient::Linear {
            angle_deg,
            opacity,
            stops,
        } => SceneGradient::Linear {
            angle_deg,
            opacity: (opacity * k).clamp(0.0, 1.0),
            stops,
        },
        SceneGradient::Radial {
            cx,
            cy,
            radius,
            opacity,
            stops,
        } => SceneGradient::Radial {
            cx,
            cy,
            radius,
            opacity: (opacity * k).clamp(0.0, 1.0),
            stops,
        },
        SceneGradient::Mesh {
            rows,
            cols,
            colors,
            opacity,
        } => SceneGradient::Mesh {
            rows,
            cols,
            colors,
            opacity: (opacity * k).clamp(0.0, 1.0),
        },
    }
}

fn text_align_to_scene(value: &str) -> SceneTextAlign {
    match value {
        "center" => SceneTextAlign::Center,
        "right" => SceneTextAlign::Right,
        "justify" => SceneTextAlign::Justify,
        _ => SceneTextAlign::Left,
    }
}

fn text_vertical_align_to_scene(value: &str) -> SceneTextVerticalAlign {
    match value {
        "middle" => SceneTextVerticalAlign::Middle,
        "bottom" => SceneTextVerticalAlign::Bottom,
        _ => SceneTextVerticalAlign::Top,
    }
}

fn image_fit_to_scene(value: Option<&str>) -> SceneImageFit {
    match value {
        Some("fit") => SceneImageFit::Fit,
        Some("crop") => SceneImageFit::Crop,
        Some("tile") => SceneImageFit::Tile,
        Some("stretch") => SceneImageFit::Stretch,
        _ => SceneImageFit::Fill,
    }
}

/// Resolve every canonical fill without collapsing the stack. Canonical arrays
/// are front-to-back; the scene preserves that order and painters reverse it.
/// Layer alpha stays authored; `SceneNode::opacity` owns direct paint opacity
/// and any isolated local group alpha lives on `SceneNode::composite_opacity`.
fn fill_layers_to_scene(
    fills: &[jian_ops_schema::style::PenFill],
    variable_fill: Option<Color>,
    size: [f32; 2],
) -> Vec<SceneFillLayer> {
    use jian_ops_schema::style::PenFill;

    if fills.is_empty() {
        return variable_fill
            .map(|color| SceneFillLayer::Solid {
                color,
                blend_mode: ImageBlendMode::Normal,
            })
            .into_iter()
            .collect();
    }

    fills
        .iter()
        .filter(|fill| fill.is_enabled())
        .enumerate()
        .filter_map(|(index, fill)| {
            let blend_mode = fill_blend_mode_to_scene(fill);
            // Override only the primary paint; retain its compositing metadata
            // and every layer behind it.
            if index == 0 {
                if let Some(color) = variable_fill {
                    return Some(SceneFillLayer::Solid {
                        color: mul_alpha(color, fill_opacity(fill)),
                        blend_mode,
                    });
                }
            }
            let fallback = || {
                crate::style_payload::fill_fallback_color(fill)
                    .map(array_to_color)
                    .map(|color| SceneFillLayer::Solid { color, blend_mode })
            };
            match fill {
                PenFill::Solid(_) => fallback(),
                PenFill::LinearGradient(_)
                | PenFill::RadialGradient(_)
                | PenFill::MeshGradient(_) => crate::style_payload::gradient_payload(fill)
                    .map(owned::payload_gradient_into_scene)
                    .map(|gradient| SceneFillLayer::Gradient {
                        gradient,
                        blend_mode,
                    })
                    .or_else(|| {
                        crate::style_payload::shader_payload(fill)
                            .map(|shader| owned::payload_shader_into_scene(shader, 1.0, size))
                            .map(|shader| SceneFillLayer::Shader { shader, blend_mode })
                    })
                    .or_else(fallback),
                PenFill::Shader(_) => crate::style_payload::shader_payload(fill)
                    .map(|shader| owned::payload_shader_into_scene(shader, 1.0, size))
                    .map(|shader| SceneFillLayer::Shader { shader, blend_mode })
                    .or_else(fallback),
                PenFill::Image(body) if !body.url.trim().is_empty() => {
                    let fit = crate::style_payload::image_fill_mode_to_payload(body.mode.as_ref());
                    Some(SceneFillLayer::Image {
                        geometry: body.geometry.as_deref().map(|g| Arc::new(g.clone())),
                        paint: Some(Arc::new((**body).clone())),
                        src: body.url.as_arc(),
                        src_id: stable_image_source_id(body.url.as_ref()),
                        thumbnail_src: body.thumbnail_url.as_ref().map(|src| src.as_arc()),
                        thumbnail_src_id: body
                            .thumbnail_url
                            .as_deref()
                            .map(stable_image_source_id)
                            .unwrap_or(0),
                        nearest_sampling: body.nearest_sampling.unwrap_or(false),
                        rotation_degrees: body.rotation.map(|value| value as f32),
                        fit: image_fit_to_scene(Some(&fit)),
                        transform: body.transform.as_ref().map(|m| {
                            [m.m00, m.m01, m.m02, m.m10, m.m11, m.m12].map(|value| value as f32)
                        }),
                        original_size: image_original_size_to_scene(
                            body.original_size
                                .as_ref()
                                .map(|size| [size.width as f32, size.height as f32]),
                        ),
                        tile_scale: image_tile_scale_to_scene(body.tile_scale.map(|s| s as f32)),
                        adjustments: image_adjustments_to_scene(
                            crate::style_payload::image_fill_adjustments(body),
                        ),
                        opacity: (body.opacity.unwrap_or(1.0) as f32).clamp(0.0, 1.0),
                        blend_mode,
                    })
                }
                PenFill::Image(_) => None,
            }
        })
        .collect()
}

fn image_original_size_to_scene(value: Option<[f32; 2]>) -> Option<[f32; 2]> {
    value.filter(|[width, height]| {
        width.is_finite() && height.is_finite() && *width > 0.0 && *height > 0.0
    })
}

fn image_tile_scale_to_scene(value: Option<f32>) -> f32 {
    value
        .filter(|scale| scale.is_finite() && *scale > 0.0)
        .unwrap_or(1.0)
}

fn fill_opacity(fill: &jian_ops_schema::style::PenFill) -> f32 {
    use jian_ops_schema::style::PenFill;
    let opacity = match fill {
        PenFill::Solid(body) => body.opacity,
        PenFill::LinearGradient(body) => body.opacity,
        PenFill::RadialGradient(body) => body.opacity,
        PenFill::MeshGradient(body) => body.opacity,
        PenFill::Shader(body) => body.opacity,
        PenFill::Image(body) => body.opacity,
    };
    (opacity.unwrap_or(1.0) as f32).clamp(0.0, 1.0)
}

fn fill_blend_mode_to_scene(fill: &jian_ops_schema::style::PenFill) -> ImageBlendMode {
    use jian_ops_schema::style::PenFill;
    let blend = match fill {
        PenFill::Solid(body) => body.blend_mode.as_ref(),
        PenFill::LinearGradient(body) => body.blend_mode.as_ref(),
        PenFill::RadialGradient(body) => body.blend_mode.as_ref(),
        PenFill::MeshGradient(body) => body.blend_mode.as_ref(),
        PenFill::Shader(body) => body.blend_mode.as_ref(),
        PenFill::Image(body) => body.blend_mode.as_ref(),
    };
    blend_mode_to_scene(blend)
}

fn blend_mode_to_scene(value: Option<&jian_ops_schema::style::BlendMode>) -> ImageBlendMode {
    use jian_ops_schema::style::BlendMode;
    match value {
        Some(BlendMode::Darken) => ImageBlendMode::Darken,
        Some(BlendMode::Multiply) => ImageBlendMode::Multiply,
        Some(BlendMode::Screen) => ImageBlendMode::Screen,
        Some(BlendMode::Overlay) => ImageBlendMode::Overlay,
        Some(BlendMode::Lighten) => ImageBlendMode::Lighten,
        Some(BlendMode::Difference) => ImageBlendMode::Difference,
        Some(BlendMode::Hue) => ImageBlendMode::Hue,
        Some(BlendMode::Saturation) => ImageBlendMode::Saturation,
        Some(BlendMode::Color) => ImageBlendMode::Color,
        Some(BlendMode::Luminosity) => ImageBlendMode::Luminosity,
        Some(BlendMode::SoftLight) => ImageBlendMode::SoftLight,
        Some(BlendMode::ColorDodge) => ImageBlendMode::ColorDodge,
        Some(BlendMode::ColorBurn) => ImageBlendMode::ColorBurn,
        Some(BlendMode::HardLight) => ImageBlendMode::HardLight,
        Some(BlendMode::Exclusion) => ImageBlendMode::Exclusion,
        Some(BlendMode::Normal) | None => ImageBlendMode::Normal,
    }
}

fn image_adjustments_to_scene(
    value: Option<crate::payload::ImageAdjustmentPayload>,
) -> op_editor_core::render_backend::ImageAdjustments {
    let Some(value) = value else {
        return op_editor_core::render_backend::ImageAdjustments::default();
    };
    op_editor_core::render_backend::ImageAdjustments {
        exposure: value.exposure,
        contrast: value.contrast,
        saturation: value.saturation,
        temperature: value.temperature,
        tint: value.tint,
        highlights: value.highlights,
        shadows: value.shadows,
    }
}

/// Convert a payload path anchor into a scene anchor.
fn anchor_to_scene(a: &crate::payload::AnchorPayload) -> jian_scene::layout_scene::SceneAnchor {
    use jian_scene::layout_scene::{SceneAnchor, ScenePointType};
    use op_editor_core::render_backend::Point2D;
    SceneAnchor {
        pos: Point2D::new(a.x, a.y),
        handle_in: a.handle_in.map(|h| Point2D::new(h[0], h[1])),
        handle_out: a.handle_out.map(|h| Point2D::new(h[0], h[1])),
        point_type: match a.point_type {
            1 => ScenePointType::Mirrored,
            2 => ScenePointType::Independent,
            _ => ScenePointType::Corner,
        },
    }
}

/// `[r, g, b, a]` payload colour → shell-core `Color`. Lossless;
/// the same conversion `apply_payload` runs on the `Document` path.
fn array_to_color(a: [f32; 4]) -> Color {
    Color {
        r: a[0],
        g: a[1],
        b: a[2],
        a: a[3],
    }
}

/// `NodePayload.kind` string → shell-core `NodeKind`. Mirrors
/// `payload::str_to_kind` so the scene's per-kind paint dispatch
/// matches the `Document` path exactly.
pub(crate) fn str_to_kind(s: &str) -> NodeKind {
    match s {
        "frame" => NodeKind::Frame,
        "group" => NodeKind::Group,
        "rect" => NodeKind::Rect,
        "ellipse" => NodeKind::Ellipse,
        "polygon" => NodeKind::Polygon,
        "line" => NodeKind::Line,
        "text" => NodeKind::Text,
        "path" => NodeKind::Path,
        other => NodeKind::Other(other.to_string()),
    }
}

fn stop_to_scene(s: &GradientStopPayload) -> SceneGradientStop {
    SceneGradientStop {
        offset: s.offset,
        color: array_to_color(s.color),
    }
}

/// Map `NodePayload.fill_type` via `str_to_fill_type` / `fill_type_to_scene`.
fn str_to_scene_fill_type(s: &str) -> SceneFillType {
    match s {
        "linear" => SceneFillType::LinearGradient,
        "radial" => SceneFillType::RadialGradient,
        "mesh" => SceneFillType::MeshGradient,
        "shader" => SceneFillType::Shader,
        "image" => SceneFillType::Image,
        _ => SceneFillType::Solid,
    }
}

#[cfg(test)]
#[path = "layout_scene_tests.rs"]
mod tests;
