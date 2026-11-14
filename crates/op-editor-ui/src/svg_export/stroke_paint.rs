//! Independent stroke paint resources and exact vector silhouettes.
use super::*;
use crate::layout_scene::{SceneFillLayer, SceneStrokeAlign};
use crate::widgets::canvas_viewport_stroke_paint::geometry;
use crate::{StrokeCurve, StrokeGeometry, StrokePaintCap, StrokePaintJoin};
fn area(node: &SceneNode) -> Rect {
    let s = node.stroke.unwrap();
    let p = s.width * 4.0 + 1.0;
    Rect::xywh(
        node.bounds.origin.x - p,
        node.bounds.origin.y - p,
        node.bounds.size.x + 2.0 * p,
        node.bounds.size.y + 2.0 * p,
    )
}
fn rect(out: &mut String, r: Rect, attrs: &str) {
    let _ = write!(
        out,
        r#"<rect x="{}" y="{}" width="{}" height="{}" {attrs}/>"#,
        r.origin.x, r.origin.y, r.size.x, r.size.y
    );
}
fn shape(out: &mut String, node: &SceneNode, attrs: &str) {
    let resolved = geometry(node, node.bounds, 1.0);
    match resolved.get() {
        StrokeGeometry::RawSvg { data, even_odd } => {
            let _ = write!(
                out,
                r#"<path d="{}" fill-rule="{}" {attrs}/>"#,
                xml_escape(data),
                if even_odd { "evenodd" } else { "nonzero" }
            );
        }
        StrokeGeometry::RoundedRect { rect: r, radii } => {
            let d = rounded(r, radii.map(|v| [v, v]));
            let _ = write!(out, r#"<path d="{d}" {attrs}/>"#);
        }
        StrokeGeometry::Oval(r) => {
            let _ = write!(
                out,
                r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" {attrs}/>"#,
                r.origin.x + r.size.x * 0.5,
                r.origin.y + r.size.y * 0.5,
                r.size.x * 0.5,
                r.size.y * 0.5
            );
        }
        StrokeGeometry::Svg {
            data,
            rect,
            even_odd,
        } => {
            let transform = svg_path_fit_transform(data, rect);
            let rule = if even_odd { "evenodd" } else { "nonzero" };
            let _ = write!(
                out,
                r#"<path d="{}"{transform} fill-rule="{rule}" vector-effect="non-scaling-stroke" {attrs}/>"#,
                xml_escape(data)
            );
        }
        StrokeGeometry::Curves { elements, even_odd } => {
            let mut d = String::new();
            for e in elements {
                match e {
                    StrokeCurve::Move(p) => {
                        let _ = write!(d, "M{} {}", p.x, p.y);
                    }
                    StrokeCurve::Line(p) => {
                        let _ = write!(d, "L{} {}", p.x, p.y);
                    }
                    StrokeCurve::Quad(a, b) => {
                        let _ = write!(d, "Q{} {} {} {}", a.x, a.y, b.x, b.y);
                    }
                    StrokeCurve::Cubic(a, b, c) => {
                        let _ = write!(d, "C{} {} {} {} {} {}", a.x, a.y, b.x, b.y, c.x, c.y);
                    }
                    StrokeCurve::Close => d.push('Z'),
                }
            }
            let _ = write!(
                out,
                r#"<path d="{d}" fill-rule="{}" {attrs}/>"#,
                if even_odd { "evenodd" } else { "nonzero" }
            );
        }
    }
}
fn style(node: &SceneNode, width: f32) -> String {
    let p = &node.stroke_paint.as_ref().unwrap().style;
    let cap = match p.cap {
        StrokePaintCap::Butt => "butt",
        StrokePaintCap::Round => "round",
        StrokePaintCap::Square => "square",
    };
    let join = match p.join {
        StrokePaintJoin::Miter => "miter",
        StrokePaintJoin::Round => "round",
        StrokePaintJoin::Bevel => "bevel",
    };
    let mut attrs = format!(
        r#"fill="none" stroke="white" stroke-width="{width}" stroke-linecap="{cap}" stroke-linejoin="{join}" stroke-miterlimit="4""#
    );
    if !p.dash_pattern.is_empty() {
        let _ = write!(
            attrs,
            r#" stroke-dasharray="{}" stroke-dashoffset="{}""#,
            p.dash_pattern
                .iter()
                .map(f32::to_string)
                .collect::<Vec<_>>()
                .join(" "),
            p.dash_offset
        );
    }
    attrs
}
pub(super) fn emit_defs(out: &mut String, node: &SceneNode) {
    emit_defs_with_key(out, node, &svg_id(&node.id));
}
pub(super) fn emit_defs_with_key(out: &mut String, node: &SceneNode, id: &str) {
    let (Some(stroke), Some(paint)) = (node.stroke, node.stroke_paint.as_deref()) else {
        return;
    };
    let mut bounds = normalize_rect(node.bounds);
    if bounds.size.x == 0.0 {
        bounds.size.x = 1.0;
    }
    if bounds.size.y == 0.0 {
        bounds.size.y = 1.0;
    }
    let a = area(node);
    let _ = write!(
        out,
        r#"<clipPath id="stroke-interior-{id}" clipPathUnits="userSpaceOnUse">"#
    );
    closed_shape(out, node, r#"fill="white" stroke="none""#);
    out.push_str("</clipPath>");
    let _ = write!(
        out,
        r#"<mask id="stroke-exterior-{id}" maskUnits="userSpaceOnUse" maskContentUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}" mask-type="luminance" color-interpolation="sRGB">"#,
        a.origin.x, a.origin.y, a.size.x, a.size.y
    );
    rect(out, a, r#"fill="white""#);
    closed_shape(out, node, r#"fill="black" stroke="none""#);
    out.push_str("</mask>");
    let _ = write!(
        out,
        r#"<mask id="stroke-coverage-{id}" maskUnits="userSpaceOnUse" maskContentUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}" mask-type="alpha">"#,
        a.origin.x, a.origin.y, a.size.x, a.size.y
    );
    if let Some(sides) = stroke.sides.filter(|s| s.iter().any(|v| *v != s[0])) {
        if matches!(
            node.kind,
            NodeKind::Rect | NodeKind::Frame | NodeKind::Text | NodeKind::Other(_)
        ) {
            let ring = side_ring(node, sides);
            if paint.style.dash_pattern.is_empty() {
                let _ = write!(
                    out,
                    r#"<path d="{ring}" fill="white" fill-rule="evenodd"/>"#
                );
            } else {
                let _ = write!(
                    out,
                    r#"<clipPath id="stroke-sides-{id}"><path d="{ring}" clip-rule="evenodd"/></clipPath><g clip-path="url(#stroke-sides-{id})">"#
                );
                shape(out, node, &style(node, stroke.width * 2.0));
                out.push_str("</g>");
            }
        } else {
            coverage(out, node, stroke.width, id);
        }
    } else {
        coverage(out, node, stroke.width, id);
    }
    out.push_str("</mask>");
    for (i, layer) in paint.layers.iter().enumerate() {
        let key = format!("stroke-paint-{id}-{i}");
        match layer {
            SceneFillLayer::Gradient { gradient, .. } => emit_gradient(out, &key, gradient, bounds),
            SceneFillLayer::Shader { shader, .. } => {
                if let Some(paint) = &shader.gradient {
                    gradient_geometry::emit(out, &key, paint, bounds, shader.opacity);
                }
            }
            _ => {}
        }
    }
}
fn path_piece(out: &mut String, node: &SceneNode, data: &str, original: &str, attrs: &str) {
    if data.is_empty() {
        return;
    }
    let transform = svg_path_fit_transform(original, node.bounds);
    let _ = write!(
        out,
        r#"<path d="{}"{transform} fill-rule="{}" vector-effect="non-scaling-stroke" {attrs}/>"#,
        xml_escape(data),
        if node.even_odd_fill {
            "evenodd"
        } else {
            "nonzero"
        }
    );
}
fn closed_shape(out: &mut String, node: &SceneNode, attrs: &str) {
    if let Some(data) = node.svg_path.as_deref() {
        if let Some((closed, _)) = op_editor_core::svg_path_split_closed_contours(data) {
            path_piece(out, node, &closed, data, attrs);
        }
    } else {
        shape(out, node, attrs);
    }
}
fn coverage(out: &mut String, node: &SceneNode, width: f32, id: &str) {
    let align = node.stroke.unwrap().align;
    if align == SceneStrokeAlign::Center {
        shape(out, node, &style(node, width));
        return;
    }
    let attr = if align == SceneStrokeAlign::Inside {
        format!(r#"clip-path="url(#stroke-interior-{id})""#)
    } else {
        format!(r#"mask="url(#stroke-exterior-{id})""#)
    };
    if let Some(data) = node.svg_path.as_deref() {
        if let Some((closed, open)) = op_editor_core::svg_path_split_closed_contours(data) {
            let _ = write!(out, "<g {attr}>");
            path_piece(out, node, &closed, data, &style(node, width * 2.0));
            out.push_str("</g>");
            path_piece(out, node, &open, data, &style(node, width));
        }
    } else if matches!(node.kind, NodeKind::Line)
        || (node.kind == NodeKind::Path && !node.path_closed)
    {
        shape(out, node, &style(node, width));
    } else {
        let _ = write!(out, "<g {attr}>");
        shape(out, node, &style(node, width * 2.0));
        out.push_str("</g>");
    }
}
pub(super) fn emit(out: &mut String, node: &SceneNode) {
    emit_with_key(out, node, &svg_id(&node.id));
}
pub(super) fn emit_with_key(out: &mut String, node: &SceneNode, id: &str) {
    let Some(paint) = &node.stroke_paint else {
        return;
    };
    let a = area(node);
    let _ = write!(
        out,
        r#"<g mask="url(#stroke-coverage-{id})" opacity="{}" style="isolation:isolate">"#,
        node.opacity
    );
    for (i, layer) in paint.layers.iter().enumerate().rev() {
        let g = render::emit_composite_start(out, 1.0, layer.blend_mode());
        let key = format!("stroke-paint-{id}-{i}");
        match layer {
            SceneFillLayer::Solid { color, .. } => rect(
                out,
                a,
                &format!(
                    r#"fill="{}" fill-opacity="{}""#,
                    color_to_rgb(*color),
                    color.a
                ),
            ),
            SceneFillLayer::Gradient { .. } => rect(out, a, &format!(r#"fill="url(#{key})""#)),
            SceneFillLayer::Shader { shader, .. } => {
                if shader.gradient.is_some() {
                    rect(out, a, &format!(r#"fill="url(#{key})""#));
                } else {
                    let mut c = shader.fallback;
                    c.a *= shader.opacity;
                    rect(
                        out,
                        a,
                        &format!(r#"fill="{}" fill-opacity="{}""#, color_to_rgb(c), c.a),
                    );
                }
            }
            SceneFillLayer::Image {
                src,
                fit,
                transform,
                opacity,
                original_size,
                rotation_degrees,
                tile_scale,
                adjustments,
                ..
            } => {
                let mut image = SceneNode::leaf("", NodeKind::Rect);
                image.bounds = node.bounds;
                image.image_src = Some(src.clone());
                image.image_fit = *fit;
                image.image_transform = *transform;
                image.image_original_size = *original_size;
                image.image_rotation_degrees = *rotation_degrees;
                image.image_tile_scale = *tile_scale;
                image.image_adjustments = *adjustments;
                image.opacity = *opacity;
                // The image resource transform is anchored to the node, while
                // the stroke mask supplies the final coverage.
                if *fit == SceneImageFit::Tile {
                    emit_image_pattern(out, &image, src, &key);
                    rect(
                        out,
                        a,
                        &format!(r#"fill="url(#image-pattern-{key})" opacity="{opacity}""#),
                    );
                } else {
                    let r = node.bounds;
                    let preserve = match fit {
                        SceneImageFit::Fit => "xMidYMid meet",
                        SceneImageFit::Fill | SceneImageFit::Crop => "xMidYMid slice",
                        _ => "none",
                    };
                    let _ = write!(
                        out,
                        r#"<image x="{}" y="{}" width="{}" height="{}" href="{}" preserveAspectRatio="{preserve}" opacity="{opacity}"/>"#,
                        r.origin.x,
                        r.origin.y,
                        r.size.x,
                        r.size.y,
                        xml_escape(src)
                    );
                }
            }
        }
        if g {
            out.push_str("</g>");
        }
    }
    out.push_str("</g>");
}
fn rounded(r: Rect, mut c: [[f32; 2]; 4]) -> String {
    let factor = [
        1.0,
        r.size.x / (c[0][0] + c[1][0]),
        r.size.x / (c[3][0] + c[2][0]),
        r.size.y / (c[0][1] + c[3][1]),
        r.size.y / (c[1][1] + c[2][1]),
    ]
    .into_iter()
    .fold(1.0f32, f32::min)
    .max(0.0);
    for corner in &mut c {
        for v in corner {
            *v = v.max(0.0) * factor;
        }
    }
    let (x, y, w, h) = (r.origin.x, r.origin.y, r.size.x, r.size.y);
    let mut d = format!("M{} {y} H{}", x + c[0][0], x + w - c[1][0]);
    let mut arc = |radius: [f32; 2], a: f32, b: f32, axis: char, edge: f32| {
        let _ = write!(
            d,
            " A{} {} 0 0 1 {a} {b} {axis}{edge}",
            radius[0], radius[1]
        );
    };
    arc(c[1], x + w, y + c[1][1], 'V', y + h - c[2][1]);
    arc(c[2], x + w - c[2][0], y + h, 'H', x + c[3][0]);
    arc(c[3], x, y + h - c[3][1], 'V', y + c[0][1]);
    let _ = write!(d, " A{} {} 0 0 1 {} {y} Z", c[0][0], c[0][1], x + c[0][0]);
    d
}
fn side_ring(node: &SceneNode, s: [f32; 4]) -> String {
    let r = node.bounds;
    let c = node.corner_radii.unwrap_or([node.corner_radius; 4]);
    let (outer, inner) = match node.stroke.unwrap().align {
        SceneStrokeAlign::Inside => (0.0, 1.0),
        SceneStrokeAlign::Center => (0.5, 0.5),
        SceneStrokeAlign::Outside => (1.0, 0.0),
    };
    let [t, ri, b, l] = s.map(|v| v.max(0.0));
    let corner = |base: f32, inset: f32, k: f32| {
        if base == 0.0
            && k > 0.0
            && node.stroke_paint.as_ref().unwrap().style.join != StrokePaintJoin::Round
        {
            0.0
        } else {
            base + inset * k
        }
    };
    let radii = |k: f32| {
        [
            [corner(c[0], l, k), corner(c[0], t, k)],
            [corner(c[1], ri, k), corner(c[1], t, k)],
            [corner(c[2], ri, k), corner(c[2], b, k)],
            [corner(c[3], l, k), corner(c[3], b, k)],
        ]
    };
    let mut d = rounded(
        Rect::xywh(
            r.origin.x - l * outer,
            r.origin.y - t * outer,
            r.size.x + (l + ri) * outer,
            r.size.y + (t + b) * outer,
        ),
        radii(outer),
    );
    let inside = Rect::xywh(
        r.origin.x + l * inner,
        r.origin.y + t * inner,
        r.size.x - (l + ri) * inner,
        r.size.y - (t + b) * inner,
    );
    if inside.size.x > 0.0 && inside.size.y > 0.0 {
        d.push_str(&rounded(inside, radii(-inner)));
    }
    d
}
