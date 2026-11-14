//! Full stroke paint stacks, sharing fill compositing and exact curve geometry.
use crate::layout_scene::{NodeKind, SceneFillLayer, SceneNode, SceneStrokeAlign};
use crate::widgets::{canvas_viewport_fill_layers::paint_clipped_shape_rich_fill_layer, PaintCx};
use crate::{
    Color, ImageBlendMode, Point2D, Rect, StrokeCurve, StrokeGeometry, StrokeMaskOptions,
    StrokePlacement,
};
use std::fmt::Write;

pub(crate) enum Geometry<'a> {
    Borrowed(StrokeGeometry<'a>),
    Curves(Vec<StrokeCurve>, bool),
    Svg(String, bool),
}
impl Geometry<'_> {
    pub(crate) fn get(&self) -> StrokeGeometry<'_> {
        match self {
            Self::Borrowed(g) => *g,
            Self::Curves(e, even_odd) => StrokeGeometry::Curves {
                elements: e,
                even_odd: *even_odd,
            },
            Self::Svg(d, e) => StrokeGeometry::RawSvg {
                data: d,
                even_odd: *e,
            },
        }
    }
}

pub(super) fn paint(cx: &mut PaintCx<'_>, node: &SceneNode, rect: Rect, zoom: f32) -> bool {
    if node.stroke_paint.is_none() || !cx.backend.supports_stroke_paint_fields() {
        return false;
    }
    let geometry = geometry(node, rect, zoom);
    paint_geometry(cx, node, rect, zoom, &geometry)
}
pub(super) fn paint_widget(
    cx: &mut PaintCx<'_>,
    node: &SceneNode,
    rect: Rect,
    radius: f32,
    zoom: f32,
) -> bool {
    paint_geometry(
        cx,
        node,
        rect,
        zoom,
        &Geometry::Borrowed(StrokeGeometry::RoundedRect {
            rect,
            radii: [radius; 4],
        }),
    )
}
fn paint_geometry(
    cx: &mut PaintCx<'_>,
    node: &SceneNode,
    rect: Rect,
    zoom: f32,
    geometry: &Geometry<'_>,
) -> bool {
    let (Some(stroke), Some(paint)) = (node.stroke, node.stroke_paint.as_deref()) else {
        return false;
    };
    if !cx.backend.supports_stroke_paint_fields() {
        return false;
    }
    let mut style = paint.style.clone();
    for d in &mut style.dash_pattern {
        *d *= zoom;
    }
    style.dash_offset *= zoom;
    let options = StrokeMaskOptions {
        width: stroke.width * zoom,
        sides: stroke.sides.map(|s| s.map(|v| v * zoom)),
        placement: match stroke.align {
            SceneStrokeAlign::Inside => StrokePlacement::Inside,
            SceneStrokeAlign::Center => StrokePlacement::Center,
            SceneStrokeAlign::Outside => StrokePlacement::Outside,
        },
        style: &style,
    };
    if let [SceneFillLayer::Solid {
        color,
        blend_mode: ImageBlendMode::Normal,
    }] = paint.layers.as_slice()
    {
        let mut color = *color;
        color.a *= node.opacity;
        return cx.backend.stroke_geometry(geometry.get(), &options, color);
    }
    if paint.layers.is_empty() {
        return true;
    }
    // A miter can extend up to four half-widths beyond a path. Include caps,
    // outside placement and AA without relocating the paint's coordinate frame.
    let padding = options.width * 4.0 + 1.0;
    let area = Rect::xywh(
        rect.origin.x - padding,
        rect.origin.y - padding,
        rect.size.x + padding * 2.0,
        rect.size.y + padding * 2.0,
    );
    cx.backend
        .push_composite_layer(area, node.opacity, ImageBlendMode::Normal);
    let paint_rect = Rect::xywh(
        rect.origin.x,
        rect.origin.y,
        if rect.size.x == 0.0 { 1.0 } else { rect.size.x },
        if rect.size.y == 0.0 { 1.0 } else { rect.size.y },
    );
    cx.backend.push_paint_space(paint_rect);
    for layer in paint.layers.iter().rev() {
        let isolated = layer.blend_mode() != ImageBlendMode::Normal;
        if isolated {
            cx.backend
                .push_composite_layer(area, 1.0, layer.blend_mode());
        }
        if let SceneFillLayer::Solid { color, .. } = layer {
            cx.backend.fill_rect(area, *color);
        } else {
            paint_clipped_shape_rich_fill_layer(cx, node, layer, area, zoom);
        }
        if isolated {
            cx.backend.restore();
        }
    }
    cx.backend.pop_paint_space();
    cx.backend.push_mask_source_layer(false);
    cx.backend
        .stroke_geometry(geometry.get(), &options, Color::WHITE);
    cx.backend.restore();
    cx.backend.restore();
    true
}

pub(crate) fn geometry(node: &SceneNode, rect: Rect, zoom: f32) -> Geometry<'_> {
    if let Some(data) = node.svg_path.as_deref() {
        return Geometry::Borrowed(StrokeGeometry::Svg {
            data,
            rect,
            even_odd: node.even_odd_fill,
        });
    }
    let world = |p: Point2D| {
        Point2D::new(
            rect.origin.x + (p.x - node.bounds.origin.x) * zoom,
            rect.origin.y + (p.y - node.bounds.origin.y) * zoom,
        )
    };
    match node.kind {
        NodeKind::Ellipse => {
            let start = node.arc_start_angle.unwrap_or(0.0);
            let sweep = node.arc_sweep_angle.unwrap_or(360.0);
            let inner = node.arc_inner_radius.unwrap_or(0.0).clamp(0.0, 1.0);
            if sweep.abs() >= 359.9 && inner <= 0.001 {
                Geometry::Borrowed(StrokeGeometry::Oval(rect))
            } else {
                Geometry::Svg(arc_data(rect, start, sweep, inner), true)
            }
        }
        NodeKind::Polygon => {
            let p = crate::layout_scene::regular_polygon_points(rect, node.polygon_sides);
            Geometry::Curves(polyline(&p, true), node.even_odd_fill)
        }
        NodeKind::Line => Geometry::Curves(
            vec![
                StrokeCurve::Move(rect.origin),
                StrokeCurve::Line(Point2D::new(
                    rect.origin.x + rect.size.x,
                    rect.origin.y + rect.size.y,
                )),
            ],
            false,
        ),
        NodeKind::Path => {
            let mut curves = Vec::new();
            let anchors = &node.path_anchors;
            if anchors.len() >= 2 {
                curves.push(StrokeCurve::Move(world(anchors[0].pos)));
                let mut segment =
                    |a: &crate::layout_scene::SceneAnchor, b: &crate::layout_scene::SceneAnchor| {
                        if a.handle_out.is_some() || b.handle_in.is_some() {
                            curves.push(StrokeCurve::Cubic(
                                world(a.handle_out.unwrap_or(a.pos)),
                                world(b.handle_in.unwrap_or(b.pos)),
                                world(b.pos),
                            ));
                        } else {
                            curves.push(StrokeCurve::Line(world(b.pos)));
                        }
                    };
                for pair in anchors.windows(2) {
                    segment(&pair[0], &pair[1]);
                }
                if node.path_closed {
                    segment(anchors.last().unwrap(), &anchors[0]);
                    curves.push(StrokeCurve::Close);
                }
            } else {
                let p: Vec<_> = node.points.iter().copied().map(world).collect();
                curves = polyline(&p, node.path_closed);
            }
            Geometry::Curves(curves, node.even_odd_fill)
        }
        _ => Geometry::Borrowed(StrokeGeometry::RoundedRect {
            rect,
            radii: node
                .corner_radii
                .unwrap_or([node.corner_radius; 4])
                .map(|r| r * zoom),
        }),
    }
}
fn polyline(p: &[Point2D], closed: bool) -> Vec<StrokeCurve> {
    let mut out = Vec::with_capacity(p.len() + 1);
    if let Some(first) = p.first() {
        out.push(StrokeCurve::Move(*first));
        out.extend(p[1..].iter().copied().map(StrokeCurve::Line));
        if closed {
            out.push(StrokeCurve::Close);
        }
    }
    out
}
fn arc_data(rect: Rect, start: f32, sweep: f32, inner: f32) -> String {
    let mut d = String::new();
    let rx = rect.size.x * 0.5;
    let ry = rect.size.y * 0.5;
    let cx = rect.origin.x + rx;
    let cy = rect.origin.y + ry;
    let point = |angle: f32, r: f32| {
        let a = angle.to_radians();
        (cx + rx * r * a.cos(), cy + ry * r * a.sin())
    };
    let full = sweep.abs() >= 359.9;
    let sweep = if full { 360.0 * sweep.signum() } else { sweep };
    let (x, y) = point(start, 1.0);
    let _ = write!(d, "M{x} {y}");
    let segments = (sweep.abs() / 180.0).ceil().max(1.0) as usize;
    for i in 1..=segments {
        let (x, y) = point(start + sweep * i as f32 / segments as f32, 1.0);
        let _ = write!(d, " A{rx} {ry} 0 0 {} {x} {y}", u8::from(sweep > 0.0));
    }
    if inner > 0.0 {
        let (x, y) = point(start + sweep, inner);
        let _ = write!(d, " {}{x} {y}", if full { "Z M" } else { "L" });
        for i in (0..segments).rev() {
            let (x, y) = point(start + sweep * i as f32 / segments as f32, inner);
            let _ = write!(
                d,
                " A{} {} 0 0 {} {x} {y}",
                rx * inner,
                ry * inner,
                u8::from(sweep < 0.0)
            );
        }
    } else if !full {
        let _ = write!(d, " L{cx} {cy}");
    }
    d.push('Z');
    d
}
