//! Platform-neutral active-page SVG serializer for native and wasm hosts.

use crate::layout_scene::{
    regular_polygon_points, LayoutScene, NodeKind, SceneGradient, SceneImageFit, SceneNode,
    ScenePage,
};
use crate::{Color, Rect};
use std::fmt::Write as _;

mod batch;
mod gradient_geometry;
pub use batch::SvgNodeBatch;

mod bounds;
mod context;
mod ids;
mod masks;
mod render;

mod composite;
#[cfg(test)]
mod composite_tests;
mod image_metadata;
#[path = "svg_export_rect.rs"]
mod rect;
mod transform;

use rect::{emit_rect, emit_rect_fill, emit_rect_stroke_overlay, emit_widget_frame_background};
use transform::{apply_clip_bounds, emit_affine_group_start, node_transform, svg_id};

use bounds::{collect_bounds, collect_sibling_bounds};
use context::AncestorEffect;
use ids::node_id;
use render::emit_node;

const MARGIN: f32 = 16.0;
const TEXT_DEFAULT_FONT_SIZE: f32 = 13.0;
const TEXT_DEFAULT_FILL: Color = Color {
    r: 0.08,
    g: 0.08,
    b: 0.08,
    a: 1.0,
};

/// A failed SVG serialization.
///
/// `Display` reproduces the previous ad-hoc `String` messages byte for byte,
/// and `From<SvgExportError> for String` keeps the `?` call sites in hosts
/// that still return `Result<_, String>` (e.g. `op-host-services`'s
/// `export_svg` wrappers) compiling unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SvgExportError {
    /// The scene carries no active page.
    NoActivePage,
    /// The selected content has no positive-area painted bounds.
    NothingToExport,
    /// The requested node is not on the active page.
    NodeNotFound(String),
    /// The requested node exists but paints no geometry.
    NodePaintsNothing(String),
    InvalidScale,
    UnsupportedGradientInterpolation {
        node_id: String,
    },
}

impl std::fmt::Display for SvgExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SvgExportError::NoActivePage => write!(f, "no active page"),
            SvgExportError::NothingToExport => write!(f, "nothing to export"),
            SvgExportError::NodeNotFound(id) => {
                write!(f, "node {id} not found on the active page")
            }
            SvgExportError::NodePaintsNothing(id) => write!(f, "node {id} paints nothing"),
            SvgExportError::UnsupportedGradientInterpolation{node_id}=>write!(f,"node {node_id} has a premultiplied alpha gradient that SVG cannot represent exactly"),
            SvgExportError::InvalidScale => write!(f, "SVG scale must be finite and positive"),
        }
    }
}

impl std::error::Error for SvgExportError {}

impl From<SvgExportError> for String {
    fn from(error: SvgExportError) -> String {
        error.to_string()
    }
}

/// Serialize the active page in `scene` as an SVG string.
pub fn serialize_active_page_svg(scene: &LayoutScene) -> Result<String, SvgExportError> {
    let Some(page) = scene.active_page() else {
        return Err(SvgExportError::NoActivePage);
    };
    serialize_page_svg(page, 1.0)
}

/// Serialize a borrowed page without copying the containing scene. Scaling
/// changes the output viewport; document geometry and the viewBox stay exact.
pub fn serialize_page_svg(page: &ScenePage, scale: f32) -> Result<String, SvgExportError> {
    serialize_svg(
        &page.children,
        page_bounds(page).ok_or(SvgExportError::NothingToExport)?,
        MARGIN,
        glam::Affine2::IDENTITY,
        &[],
        scale,
        true,
    )
}

/// Serialize one active-page node and subtree, tightly cropped to its painted bounds.
pub fn serialize_node_svg(scene: &LayoutScene, node_id: &str) -> Result<String, SvgExportError> {
    let page = scene.active_page().ok_or(SvgExportError::NoActivePage)?;
    serialize_node_on_page_svg(page, node_id, 1.0)
}

/// Serialize a borrowed containing-page node with its ancestor transforms and
/// clips. This also supports arbitrary-page headless export without scene clones.
pub fn serialize_node_on_page_svg(
    page: &ScenePage,
    node_id: &str,
    scale: f32,
) -> Result<String, SvgExportError> {
    SvgNodeBatch::new(page, &[node_id.to_owned()]).serialize(node_id, scale)
}

fn node_context_bounds(
    node: &SceneNode,
    ancestor_xform: glam::Affine2,
    ancestor_effects: &[AncestorEffect<'_>],
) -> Option<Rect> {
    let mut acc = BoundsAcc::new();
    collect_bounds(node, ancestor_xform, &mut acc);
    context::apply_bounds(acc.into_rect()?, ancestor_effects)
}

fn serialize_node_context(
    node: &SceneNode,
    ancestor_xform: glam::Affine2,
    ancestor_effects: &[AncestorEffect<'_>],
    bounds: Rect,
    scale: f32,
) -> Result<String, SvgExportError> {
    serialize_svg(
        std::slice::from_ref(node),
        bounds,
        0.0,
        ancestor_xform,
        ancestor_effects,
        scale,
        false,
    )
}

fn serialize_svg(
    nodes: &[SceneNode],
    bounds: Rect,
    margin: f32,
    root_xform: glam::Affine2,
    ancestor_effects: &[AncestorEffect<'_>],
    scale: f32,
    sibling_roots: bool,
) -> Result<String, SvgExportError> {
    for node in nodes {
        gradient_geometry::validate(node)?;
    }
    for effect in ancestor_effects {
        if let AncestorEffect::Mask { node, .. } = effect {
            gradient_geometry::validate(node)?;
        }
    }
    let view_x = bounds.origin.x - margin;
    let view_y = bounds.origin.y - margin;
    let view_w = bounds.size.x + margin * 2.0;
    let view_h = bounds.size.y + margin * 2.0;
    if view_w <= 0.0 || view_h <= 0.0 {
        return Err(SvgExportError::NothingToExport);
    }
    let output_w = view_w * scale;
    let output_h = view_h * scale;
    if !scale.is_finite() || scale <= 0.0 || !output_w.is_finite() || !output_h.is_finite() {
        return Err(SvgExportError::InvalidScale);
    }
    let mut svg = String::with_capacity(4096);
    let _ = write!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{view_x} {view_y} {view_w} {view_h}" width="{output_w}" height="{output_h}">"#
    );
    emit_defs(&mut svg, nodes, ancestor_effects, sibling_roots);
    context::emit_groups_start(&mut svg, ancestor_effects);
    let wraps_root = root_xform != glam::Affine2::IDENTITY;
    if wraps_root {
        emit_affine_group_start(&mut svg, root_xform);
    }
    if sibling_roots {
        masks::emit_siblings(&mut svg, nodes);
    } else {
        for node in nodes {
            emit_node(&mut svg, node);
        }
    }
    if wraps_root {
        svg.push_str("</g>");
    }
    context::close_groups(&mut svg, ancestor_effects);
    svg.push_str("</svg>");
    Ok(svg)
}

fn emit_defs(
    out: &mut String,
    nodes: &[SceneNode],
    ancestor_effects: &[AncestorEffect<'_>],
    sibling_roots: bool,
) {
    let mut defs = String::new();
    context::emit_defs(&mut defs, ancestor_effects);
    for node in nodes {
        emit_node_defs(&mut defs, node, sibling_roots);
    }
    if !defs.is_empty() {
        out.push_str("<defs>");
        out.push_str(&defs);
        out.push_str("</defs>");
    }
}

fn emit_node_defs(out: &mut String, n: &SceneNode, sibling_mask: bool) {
    if n.hidden {
        return;
    }
    let id = svg_id(&n.id);
    if let Some(shader) = &n.shader {
        if let Some(paint) = shader.gradient.as_deref() {
            gradient_geometry::emit(
                out,
                &format!("gradient-{id}"),
                paint,
                normalize_rect(n.bounds),
                shader.opacity,
            );
        }
    }
    if let Some(gradient) = &n.gradient {
        emit_gradient(
            out,
            &format!("gradient-{id}"),
            gradient,
            normalize_rect(n.bounds),
        );
    }
    let clips_image = n.image_src.is_some() && n.corner_radius > 0.0;
    if (n.clip_content || clips_image) && n.bounds.size.x != 0.0 && n.bounds.size.y != 0.0 {
        let r = normalize_rect(n.bounds);
        let _ = write!(
            out,
            r#"<clipPath id="clip-{id}"><rect x="{}" y="{}" width="{}" height="{}" rx="{}"/></clipPath>"#,
            r.origin.x,
            r.origin.y,
            r.size.x,
            r.size.y,
            n.corner_radius.max(0.0),
        );
    }
    if n.image_fit == SceneImageFit::Tile {
        if let Some(src) = n.image_src.as_deref() {
            emit_image_pattern(out, n, src, &id);
        }
    }
    for child in n.visible_children() {
        emit_node_defs(out, child, true);
    }
    if sibling_mask {
        masks::emit_definition(out, n, glam::Affine2::IDENTITY, false);
    }
}

fn emit_gradient(out: &mut String, id: &str, gradient: &SceneGradient, bounds: Rect) {
    match gradient {
        SceneGradient::Linear {
            angle_deg,
            opacity,
            stops,
        } => {
            let angle = (angle_deg - 90.0).to_radians();
            let cx = bounds.origin.x + bounds.size.x * 0.5;
            let cy = bounds.origin.y + bounds.size.y * 0.5;
            let dx = angle.cos() * bounds.size.x * 0.5;
            let dy = angle.sin() * bounds.size.y * 0.5;
            let _ = write!(
                out,
                r#"<linearGradient id="{id}" gradientUnits="userSpaceOnUse" x1="{}" y1="{}" x2="{}" y2="{}">"#,
                cx - dx,
                cy - dy,
                cx + dx,
                cy + dy,
            );
            emit_stops(out, stops, *opacity);
            out.push_str("</linearGradient>");
        }
        SceneGradient::Radial {
            cx,
            cy,
            radius,
            opacity,
            stops,
        } => {
            let center_x = bounds.origin.x + bounds.size.x * cx.clamp(0.0, 1.0);
            let center_y = bounds.origin.y + bounds.size.y * cy.clamp(0.0, 1.0);
            let outer = (bounds.size.x.max(bounds.size.y) * radius.clamp(0.0, 1.0)).max(0.01);
            let _ = write!(
                out,
                r#"<radialGradient id="{id}" gradientUnits="userSpaceOnUse" cx="{center_x}" cy="{center_y}" r="{outer}">"#
            );
            emit_stops(out, stops, *opacity);
            out.push_str("</radialGradient>");
        }
        SceneGradient::Mesh {
            colors, opacity, ..
        } => {
            if let Some(color) = colors.first() {
                let _ = write!(
                    out,
                    r#"<linearGradient id="{id}"><stop offset="0" stop-color="{}" stop-opacity="{}"/><stop offset="1" stop-color="{}" stop-opacity="{}"/></linearGradient>"#,
                    color_to_rgb(*color),
                    color.a * opacity,
                    color_to_rgb(*color),
                    color.a * opacity
                );
            }
        }
    }
}

fn emit_stops(out: &mut String, stops: &[crate::layout_scene::SceneGradientStop], opacity: f32) {
    // `SceneGradient::opacity` already includes the node's cumulative
    // layer opacity, matching the canvas shader path. Applying
    // `SceneNode::opacity` again here or on an enclosing group would
    // double-dim gradients and descendants.
    for stop in stops {
        let _ = write!(
            out,
            r#"<stop offset="{}" stop-color="{}" stop-opacity="{}"/>"#,
            stop.offset.clamp(0.0, 1.0),
            color_to_rgb(stop.color),
            stop.color.a * opacity.clamp(0.0, 1.0)
        );
    }
}

fn page_bounds(page: &ScenePage) -> Option<Rect> {
    let mut acc = BoundsAcc::new();
    collect_sibling_bounds(&page.children, glam::Affine2::IDENTITY, &mut acc);
    acc.into_rect()
}

use crate::scene_bounds::{normalize_rect, BoundsAcc, PaintCornerRules};

fn emit_ellipse(out: &mut String, n: &SceneNode) {
    let r = normalize_rect(n.bounds);
    if r.size.x == 0.0 || r.size.y == 0.0 {
        return;
    }
    let cx = r.origin.x + r.size.x * 0.5;
    let cy = r.origin.y + r.size.y * 0.5;
    let _ = write!(
        out,
        r#"<ellipse id="{}" cx="{cx}" cy="{cy}" rx="{}" ry="{}"{}/>"#,
        xml_escape(&node_id(&n.id)),
        r.size.x * 0.5,
        r.size.y * 0.5,
        fill_stroke_attrs(n),
    );
}

fn emit_polygon(out: &mut String, n: &SceneNode) {
    let r = normalize_rect(n.bounds);
    if r.size.x == 0.0 || r.size.y == 0.0 {
        return;
    }
    let points = regular_polygon_points(r, n.polygon_sides);
    let mut point_attr = String::new();
    for (i, p) in points.iter().enumerate() {
        if i > 0 {
            point_attr.push(' ');
        }
        let _ = write!(point_attr, "{},{}", p.x, p.y);
    }
    let _ = write!(
        out,
        r#"<polygon id="{}" points="{point_attr}"{}/>"#,
        xml_escape(&node_id(&n.id)),
        fill_stroke_attrs(n),
    );
}

fn emit_line(out: &mut String, n: &SceneNode) {
    let (color, width) = match n.stroke {
        Some(s) => (s.color, s.width),
        None => (n.fill.unwrap_or(Color::BLACK), 1.5),
    };
    let r = n.bounds;
    let x2 = r.origin.x + r.size.x;
    let y2 = r.origin.y + r.size.y;
    let _ = write!(
        out,
        r#"<line id="{}" x1="{}" y1="{}" x2="{x2}" y2="{y2}"{}/>"#,
        xml_escape(&node_id(&n.id)),
        r.origin.x,
        r.origin.y,
        stroke_attrs(color, width),
    );
}

fn emit_text(out: &mut String, n: &SceneNode) {
    let Some(text) = n.text.as_deref() else {
        return;
    };
    if text.is_empty() {
        return;
    }
    let r = normalize_rect(n.bounds);
    let color = n.fill.unwrap_or(TEXT_DEFAULT_FILL);
    let base_size = if n.font_size > 0.0 {
        n.font_size
    } else {
        TEXT_DEFAULT_FONT_SIZE
    };
    let line_h = base_size * 1.35;
    let fill_attr = if color.a < 0.999 {
        format!(
            r#" fill="{}" fill-opacity="{}""#,
            color_to_rgb(color),
            color.a
        )
    } else {
        format!(r#" fill="{}""#, color_to_rgb(color))
    };
    let weight_attr = if n.font_weight > 0 {
        format!(r#" font-weight="{}""#, n.font_weight)
    } else {
        String::new()
    };
    let _ = write!(out, r#"<g id="{}">"#, xml_escape(&node_id(&n.id)));
    let mut baseline_y = r.origin.y + base_size + 1.0;
    for line in text.split('\n') {
        let _ = write!(
            out,
            r#"<text x="{}" y="{baseline_y}" font-family="{}" font-size="{base_size}"{weight_attr}{fill_attr}{}{}>{}</text>"#,
            r.origin.x,
            xml_escape(if n.font_family.is_empty() {
                "system-ui, sans-serif"
            } else {
                &n.font_family
            }),
            if n.italic {
                r#" font-style="italic""#
            } else {
                ""
            },
            text_decoration_attr(n),
            xml_escape(line),
        );
        baseline_y += line_h;
    }
    out.push_str("</g>");
}

fn emit_path(out: &mut String, n: &SceneNode) {
    let (d, transform) = if let Some(d) = &n.svg_path {
        (d.clone(), svg_path_fit_transform(d, n.bounds))
    } else {
        if n.points.len() < 2 {
            return;
        }
        let mut d = String::with_capacity(n.points.len() * 16);
        let _ = write!(d, "M{} {}", n.points[0].x, n.points[0].y);
        for p in &n.points[1..] {
            let _ = write!(d, " L{} {}", p.x, p.y);
        }
        if n.path_closed {
            d.push_str(" Z");
        }
        (d, String::new())
    };
    let attrs = if n.svg_path.is_none() && !n.path_closed && n.stroke.is_none() {
        let color = n.fill.unwrap_or(Color::BLACK);
        format!(r#" fill="none"{}"#, stroke_attrs(color, 1.5))
    } else {
        fill_stroke_attrs(n)
    };
    let vector_effect = if n.svg_path.is_some() && n.stroke.is_some() {
        r#" vector-effect="non-scaling-stroke""#
    } else {
        ""
    };
    let fill_rule = if n.even_odd_fill {
        r#" fill-rule="evenodd""#
    } else {
        ""
    };
    let _ = write!(
        out,
        r#"<path id="{}" d="{}"{transform}{vector_effect}{fill_rule}{}/>"#,
        xml_escape(&node_id(&n.id)),
        xml_escape(&d),
        attrs
    );
}

fn svg_path_fit_transform(d: &str, bounds: Rect) -> String {
    let rect = normalize_rect(bounds);
    let Some((source_x, source_y, source_w, source_h)) = op_editor_core::svg_path_data_bounds(d)
    else {
        return format!(
            r#" transform="translate({} {})""#,
            rect.origin.x, rect.origin.y
        );
    };
    let sx = if source_w.abs() > 0.01 {
        rect.size.x / source_w
    } else {
        1.0
    };
    let sy = if source_h.abs() > 0.01 {
        rect.size.y / source_h
    } else {
        1.0
    };
    let tx = rect.origin.x - source_x * sx;
    let ty = rect.origin.y - source_y * sy;
    format!(r#" transform="matrix({sx} 0 0 {sy} {tx} {ty})""#)
}

fn emit_image(out: &mut String, n: &SceneNode) {
    let Some(src) = n.image_src.as_deref() else {
        return;
    };
    let r = normalize_rect(n.bounds);
    let id = svg_id(&n.id);
    let clip = if n.corner_radius > 0.0 {
        format!(r#" clip-path="url(#clip-{id})""#)
    } else {
        String::new()
    };
    if n.image_fit == SceneImageFit::Tile {
        let _ = write!(
            out,
            r#"<rect id="{}" x="{}" y="{}" width="{}" height="{}" fill="url(#image-pattern-{id})" opacity="{}"{clip}/>"#,
            xml_escape(&node_id(&n.id)),
            r.origin.x,
            r.origin.y,
            r.size.x,
            r.size.y,
            n.opacity.clamp(0.0, 1.0),
        );
        return;
    }
    let preserve = match n.image_fit {
        SceneImageFit::Fit => "xMidYMid meet",
        SceneImageFit::Crop | SceneImageFit::Fill => "xMidYMid slice",
        SceneImageFit::Stretch => "none",
        SceneImageFit::Tile => unreachable!("tile images return above"),
    };
    let _ = write!(
        out,
        r#"<image id="{}" x="{}" y="{}" width="{}" height="{}" href="{}" preserveAspectRatio="{}" opacity="{}"{clip}/>"#,
        xml_escape(&node_id(&n.id)),
        r.origin.x,
        r.origin.y,
        r.size.x,
        r.size.y,
        xml_escape(src),
        preserve,
        n.opacity.clamp(0.0, 1.0)
    );
}

fn emit_image_pattern(out: &mut String, n: &SceneNode, src: &str, id: &str) {
    let r = normalize_rect(n.bounds);
    // If source bytes are unavailable (for example an uncached remote URL),
    // fall back to one bounds-sized cell instead of inventing a repeat scale.
    let (tile_w, tile_h) = image_metadata::intrinsic_dimensions(n, src)
        .unwrap_or((r.size.x.max(1.0), r.size.y.max(1.0)));
    let start_x = r.origin.x + (r.size.x - tile_w) * 0.5;
    let start_y = r.origin.y + (r.size.y - tile_h) * 0.5;
    let _ = write!(
        out,
        r#"<pattern id="image-pattern-{id}" patternUnits="userSpaceOnUse" x="{start_x}" y="{start_y}" width="{tile_w}" height="{tile_h}" viewBox="0 0 {tile_w} {tile_h}"><image width="{tile_w}" height="{tile_h}" href="{}" preserveAspectRatio="none"/></pattern>"#,
        xml_escape(src),
    );
}

fn text_decoration_attr(n: &SceneNode) -> &'static str {
    match (n.underline, n.strikethrough) {
        (true, true) => r#" text-decoration="underline line-through""#,
        (true, false) => r#" text-decoration="underline""#,
        (false, true) => r#" text-decoration="line-through""#,
        (false, false) => "",
    }
}

#[path = "svg_export/escaped.rs"]
mod escaped;
fn xml_escape(text: &str) -> escaped::Escaped<'_> {
    escaped::Escaped(text)
}

fn fill_stroke_attrs(n: &SceneNode) -> String {
    let mut s = fill_attrs(n);
    if let Some(stroke) = n.stroke {
        s.push_str(&stroke_attrs(stroke.color, stroke.width));
    }
    s
}

fn fill_attrs(n: &SceneNode) -> String {
    let mut s = String::new();
    if n.gradient.is_some()
        || n.shader
            .as_ref()
            .is_some_and(|shader| shader.gradient.is_some())
    {
        let _ = write!(s, r#" fill="url(#gradient-{})""#, svg_id(&n.id));
    } else if let Some(fill) = n.fill {
        let _ = write!(s, r#" fill="{}""#, color_to_rgb(fill));
        if fill.a < 0.999 {
            let _ = write!(s, r#" fill-opacity="{}""#, fill.a);
        }
    } else {
        s.push_str(r#" fill="none""#);
    }
    s
}

fn stroke_attrs(color: Color, width: f32) -> String {
    let mut s = format!(
        r#" stroke="{}" stroke-width="{}""#,
        color_to_rgb(color),
        width
    );
    if color.a < 0.999 {
        let _ = write!(s, r#" stroke-opacity="{}""#, color.a);
    }
    s
}

fn color_to_rgb(c: Color) -> String {
    format!(
        "rgb({},{},{})",
        (c.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.b.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

#[cfg(test)]
#[path = "svg_export_tests.rs"]
mod tests;
