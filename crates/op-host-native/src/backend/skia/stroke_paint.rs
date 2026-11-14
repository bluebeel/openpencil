//! Full-style curve strokes for direct solids and white paint-field masks.

use super::{jian_color_to_color4f, raster_point, to_sk_rect, NativeBackend};
use op_editor_ui::{
    Color, StrokeCurve, StrokeGeometry, StrokeMaskOptions, StrokePaintCap, StrokePaintJoin,
    StrokePlacement,
};
use skia_safe::{Path, PathBuilder, PathFillType, PathVerb};

impl NativeBackend {
    pub fn stroke_geometry(
        &mut self,
        canvas: &skia_safe::Canvas,
        geometry: StrokeGeometry<'_>,
        options: &StrokeMaskOptions<'_>,
        color: Color,
    ) {
        if !options.width.is_finite() || options.width <= 0.0 {
            return;
        }
        let Some(path) = self.stroke_geometry_path(geometry) else {
            return;
        };
        let mut paint = skia_safe::Paint::new(jian_color_to_color4f(color), None);
        jian_skia::raster_sampling::configure_geometry(&mut paint);
        paint.set_stroke(true);
        paint.set_stroke_width(options.width);
        paint.set_stroke_miter(4.0);
        paint.set_stroke_cap(match options.style.cap {
            StrokePaintCap::Butt => skia_safe::PaintCap::Butt,
            StrokePaintCap::Round => skia_safe::PaintCap::Round,
            StrokePaintCap::Square => skia_safe::PaintCap::Square,
        });
        paint.set_stroke_join(match options.style.join {
            StrokePaintJoin::Miter => skia_safe::PaintJoin::Miter,
            StrokePaintJoin::Round => skia_safe::PaintJoin::Round,
            StrokePaintJoin::Bevel => skia_safe::PaintJoin::Bevel,
        });
        if !options.style.dash_pattern.is_empty() {
            paint.set_path_effect(skia_safe::PathEffect::dash(
                &options.style.dash_pattern,
                options.style.dash_offset,
            ));
        }
        if let StrokeGeometry::RoundedRect { rect, radii } = geometry {
            if let Some(sides) = options.sides.filter(|s| s.iter().any(|v| *v != s[0])) {
                let ring = side_ring(rect, radii, sides, options.placement, options.style.join);
                if options.style.dash_pattern.is_empty() {
                    paint.set_stroke(false);
                    raster_point::draw_path(canvas, &ring, &paint);
                } else {
                    let save = canvas.save();
                    canvas.clip_path(
                        &ring,
                        skia_safe::ClipOp::Intersect,
                        jian_skia::raster_sampling::geometry_antialias(),
                    );
                    paint.set_stroke_width(
                        sides.iter().copied().fold(options.width, f32::max) * 2.0,
                    );
                    raster_point::draw_path(canvas, &path, &paint);
                    canvas.restore_to_count(save);
                }
                return;
            }
        }
        if options.placement == StrokePlacement::Center {
            raster_point::draw_path(canvas, &path, &paint);
            return;
        }
        let (closed, open) = split_contours(&path);
        if !closed.is_empty() {
            let save = canvas.save();
            canvas.clip_path(
                &closed,
                if options.placement == StrokePlacement::Inside {
                    skia_safe::ClipOp::Intersect
                } else {
                    skia_safe::ClipOp::Difference
                },
                jian_skia::raster_sampling::geometry_antialias(),
            );
            paint.set_stroke_width(options.width * 2.0);
            raster_point::draw_path(canvas, &closed, &paint);
            canvas.restore_to_count(save);
        }
        // Inside/outside is meaningful for closed boundaries only. Open
        // contours retain centered widths, including mixed compound paths.
        if !open.is_empty() {
            paint.set_stroke_width(options.width);
            raster_point::draw_path(canvas, &open, &paint);
        }
    }

    fn stroke_geometry_path(&mut self, geometry: StrokeGeometry<'_>) -> Option<Path> {
        let mut out = PathBuilder::new();
        match geometry {
            StrokeGeometry::RawSvg { data, even_odd } => {
                let (_, path, _) = self.cached_svg_path(data)?;
                let mut out = PathBuilder::new();
                out.add_path(&path, None);
                if even_odd {
                    out.set_fill_type(PathFillType::EvenOdd);
                }
                return Some(out.detach());
            }

            StrokeGeometry::RoundedRect { rect, radii } => {
                out.add_rrect(rrect(to_sk_rect(rect), radii.map(|r| [r, r])), None, None);
            }
            StrokeGeometry::Oval(rect) => {
                out.add_oval(to_sk_rect(rect), None, None);
            }
            StrokeGeometry::Svg {
                data,
                rect,
                even_odd,
            } => {
                let (_, path, _) = self.cached_svg_path(data)?;
                return Some(super::path::fit_path_to_rect_with_rule(
                    &path, rect, even_odd,
                ));
            }
            StrokeGeometry::Curves { elements, even_odd } => {
                if even_odd {
                    out.set_fill_type(PathFillType::EvenOdd);
                }
                let p = |p: op_editor_ui::Point2D| skia_safe::Point::new(p.x, p.y);
                for element in elements {
                    match *element {
                        StrokeCurve::Move(a) => {
                            out.move_to(p(a));
                        }
                        StrokeCurve::Line(a) => {
                            out.line_to(p(a));
                        }
                        StrokeCurve::Quad(a, b) => {
                            out.quad_to(p(a), p(b));
                        }
                        StrokeCurve::Cubic(a, b, c) => {
                            out.cubic_to(p(a), p(b), p(c));
                        }
                        StrokeCurve::Close => {
                            out.close();
                        }
                    }
                }
            }
        }
        Some(out.detach())
    }
}

fn rrect(rect: skia_safe::Rect, radii: [[f32; 2]; 4]) -> skia_safe::RRect {
    skia_safe::RRect::new_rect_radii(
        rect,
        &radii.map(|r| skia_safe::Vector::new(r[0].max(0.0), r[1].max(0.0))),
    )
}
fn side_ring(
    rect: op_editor_ui::Rect,
    radii: [f32; 4],
    sides: [f32; 4],
    placement: StrokePlacement,
    join: StrokePaintJoin,
) -> Path {
    let sides = sides.map(|v| v.max(0.0));
    let (outside, inside) = match placement {
        StrokePlacement::Inside => (0.0, 1.0),
        StrokePlacement::Center => (0.5, 0.5),
        StrokePlacement::Outside => (1.0, 0.0),
    };
    let [top, right, bottom, left] = sides;
    let base = to_sk_rect(rect);
    let outer = skia_safe::Rect::new(
        base.left() - left * outside,
        base.top() - top * outside,
        base.right() + right * outside,
        base.bottom() + bottom * outside,
    );
    let inner = skia_safe::Rect::new(
        base.left() + left * inside,
        base.top() + top * inside,
        base.right() - right * inside,
        base.bottom() - bottom * inside,
    );
    let corner = |base: f32, inset: f32, factor: f32| {
        if base == 0.0 && factor > 0.0 && join != StrokePaintJoin::Round {
            0.0
        } else {
            base + inset * factor
        }
    };
    let expand = |factor: f32| {
        [
            [
                corner(radii[0], left, factor),
                corner(radii[0], top, factor),
            ],
            [
                corner(radii[1], right, factor),
                corner(radii[1], top, factor),
            ],
            [
                corner(radii[2], right, factor),
                corner(radii[2], bottom, factor),
            ],
            [
                corner(radii[3], left, factor),
                corner(radii[3], bottom, factor),
            ],
        ]
    };
    let mut path = PathBuilder::new_with_fill_type(PathFillType::EvenOdd);
    path.add_rrect(rrect(outer, expand(outside)), None, None);
    if inner.width() > 0.0 && inner.height() > 0.0 {
        path.add_rrect(rrect(inner, expand(-inside)), None, None);
    }
    path.detach()
}

fn split_contours(path: &Path) -> (Path, Path) {
    let mut closed = PathBuilder::new_with_fill_type(path.fill_type());
    let mut open = PathBuilder::new_with_fill_type(path.fill_type());
    let mut contour = PathBuilder::new_with_fill_type(path.fill_type());
    let mut active = false;
    for record in path.iter() {
        let p = record.points();
        match record.verb() {
            PathVerb::Move => {
                if active {
                    open.add_path(&contour.detach(), None);
                }
                contour.move_to(p[0]);
                active = true;
            }
            PathVerb::Line => {
                contour.line_to(p[1]);
            }
            PathVerb::Quad => {
                contour.quad_to(p[1], p[2]);
            }
            PathVerb::Conic => {
                contour.conic_to(p[1], p[2], record.conic_weight());
            }
            PathVerb::Cubic => {
                contour.cubic_to(p[1], p[2], p[3]);
            }
            PathVerb::Close => {
                contour.close();
                closed.add_path(&contour.detach(), None);
                active = false;
            }
        }
    }
    if active {
        open.add_path(&contour.detach(), None);
    }
    (closed.detach(), open.detach())
}
