use super::super::bindings::OpCk;
use op_editor_ui::{
    Color, StrokeCurve, StrokeGeometry, StrokeMaskOptions, StrokePaintCap, StrokePaintJoin,
    StrokePlacement,
};
pub(super) fn draw(
    ck: &OpCk,
    geometry: StrokeGeometry<'_>,
    options: &StrokeMaskOptions<'_>,
    color: Color,
) {
    let mut values = Vec::new();
    let mut data = "";
    let mut even_odd = false;
    let kind = match geometry {
        StrokeGeometry::RawSvg {
            data: d,
            even_odd: e,
        } => {
            data = d;
            even_odd = e;
            4
        }
        StrokeGeometry::RoundedRect { rect, radii } => {
            values.extend([rect.origin.x, rect.origin.y, rect.size.x, rect.size.y]);
            values.extend(radii);
            0
        }
        StrokeGeometry::Oval(rect) => {
            values.extend([rect.origin.x, rect.origin.y, rect.size.x, rect.size.y]);
            1
        }
        StrokeGeometry::Svg {
            data: d,
            rect,
            even_odd: e,
        } => {
            data = d;
            even_odd = e;
            values.extend([rect.origin.x, rect.origin.y, rect.size.x, rect.size.y]);
            2
        }
        StrokeGeometry::Curves {
            elements,
            even_odd: e,
        } => {
            even_odd = e;
            for element in elements {
                match element {
                    StrokeCurve::Move(a) => values.extend([0.0, a.x, a.y]),
                    StrokeCurve::Line(a) => values.extend([1.0, a.x, a.y]),
                    StrokeCurve::Quad(a, b) => values.extend([2.0, a.x, a.y, b.x, b.y]),
                    StrokeCurve::Cubic(a, b, c) => {
                        values.extend([4.0, a.x, a.y, b.x, b.y, c.x, c.y])
                    }
                    StrokeCurve::Close => values.push(5.0),
                }
            }
            3
        }
    };
    let placement = match options.placement {
        StrokePlacement::Inside => 0,
        StrokePlacement::Center => 1,
        StrokePlacement::Outside => 2,
    };
    let cap = match options.style.cap {
        StrokePaintCap::Butt => 0,
        StrokePaintCap::Round => 1,
        StrokePaintCap::Square => 2,
    };
    let join = match options.style.join {
        StrokePaintJoin::Miter => 0,
        StrokePaintJoin::Round => 1,
        StrokePaintJoin::Bevel => 2,
    };
    ck.stroke_geometry(
        kind,
        data,
        &values,
        even_odd,
        options.width,
        options.sides.as_ref().map_or(&[], |s| &s[..]),
        placement,
        cap,
        join,
        &options.style.dash_pattern,
        options.style.dash_offset,
        &[color.r, color.g, color.b, color.a],
    );
}
