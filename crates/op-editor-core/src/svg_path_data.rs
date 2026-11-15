//! SVG path `d` helpers used by import.
//!
//! The editable path model stores anchors. Imported SVG paths need a
//! separate preserve-`d` path to retain arc primitives and compound fill
//! rules without flattening.

mod affine;
mod arc;
mod parser;

pub(crate) use affine::{cubic_path, localize_transformed, transformed_bounds};
pub(crate) use arc::{endpoint_arc, include_arc_bounds};
pub(crate) use parser::AbsoluteSegments;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SvgPathBounds {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Segment {
    Move(f64, f64),
    Line(f64, f64),
    HLine(f64),
    VLine(f64),
    Cubic(f64, f64, f64, f64, f64, f64),
    SmoothCubic(f64, f64, f64, f64),
    Quad(f64, f64, f64, f64),
    SmoothQuad(f64, f64),
    Arc(f64, f64, f64, f64, f64, f64, f64),
    Close,
}

/// Convert any supported relative commands to absolute commands,
/// compute a tight path bbox, then shift absolute coordinates so
/// the path is local to its own bbox origin.
pub(crate) fn localize_svg_path(d: &str) -> Option<(String, SvgPathBounds)> {
    localize_svg_path_scaled(d, 1.0)
}

pub(crate) fn localize_svg_path_scaled(d: &str, scale: f64) -> Option<(String, SvgPathBounds)> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let mut segments = absolute_segments(d)?;
    if scale != 1.0 {
        for segment in &mut segments {
            *segment = segment.scaled(scale);
        }
    }
    localize_segments(&segments)
}

fn localize_segments(segments: &[Segment]) -> Option<(String, SvgPathBounds)> {
    let tight = segment_bounds(segments)?;
    let bounds = SvgPathBounds {
        x: tight.x,
        y: tight.y,
        // Renderers fit d's tight source bounds into this rectangle. Outward
        // rounding would scale fractional geometry and shift paint coordinates.
        w: if tight.w > 0.0 { tight.w } else { 1.0 },
        h: if tight.h > 0.0 { tight.h } else { 1.0 },
    };
    let local = serialize_local_segments(segments, bounds.x, bounds.y);
    Some((local, bounds))
}

pub(crate) fn svg_path_bounds(d: &str) -> Option<SvgPathBounds> {
    segment_bounds(&absolute_segments(d)?)
}

fn absolute_segments(d: &str) -> Option<Vec<Segment>> {
    let segments: Vec<_> = AbsoluteSegments::new(d).collect::<Result<_, _>>().ok()?;
    if segments.is_empty() {
        None
    } else {
        Some(segments)
    }
}

pub(crate) fn scale_svg_path(d: &str, scale: f64) -> Option<String> {
    if !scale.is_finite() {
        return None;
    }
    let mut segments = absolute_segments(d)?;
    for segment in &mut segments {
        *segment = segment.scaled(scale);
    }
    if segments.iter().any(|segment| !segment.is_finite()) {
        return None;
    }
    Some(serialize_local_segments(&segments, 0.0, 0.0))
}

impl Segment {
    fn scaled(self, scale: f64) -> Self {
        match self {
            Self::Move(x, y) => Self::Move(x * scale, y * scale),
            Self::Line(x, y) => Self::Line(x * scale, y * scale),
            Self::HLine(x) => Self::HLine(x * scale),
            Self::VLine(y) => Self::VLine(y * scale),
            Self::Cubic(a, b, c, d, x, y) => Self::Cubic(
                a * scale,
                b * scale,
                c * scale,
                d * scale,
                x * scale,
                y * scale,
            ),
            Self::SmoothCubic(a, b, x, y) => {
                Self::SmoothCubic(a * scale, b * scale, x * scale, y * scale)
            }
            Self::Quad(a, b, x, y) => Self::Quad(a * scale, b * scale, x * scale, y * scale),
            Self::SmoothQuad(x, y) => Self::SmoothQuad(x * scale, y * scale),
            Self::Arc(rx, ry, rotation, large, sweep, x, y) => Self::Arc(
                rx * scale.abs(),
                ry * scale.abs(),
                rotation,
                large,
                sweep,
                x * scale,
                y * scale,
            ),
            Self::Close => Self::Close,
        }
    }
}

fn segment_bounds(segments: &[Segment]) -> Option<SvgPathBounds> {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut include = |x: f64, y: f64| {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    };
    let (mut cx, mut cy) = (0.0f64, 0.0f64);
    let (mut sx, mut sy) = (0.0f64, 0.0f64);
    let mut last_cubic_ctrl: Option<(f64, f64)> = None;
    let mut last_quad_ctrl: Option<(f64, f64)> = None;
    for seg in segments {
        if !seg.is_finite() {
            return None;
        }
        match *seg {
            Segment::Move(x, y) => {
                cx = x;
                cy = y;
                sx = x;
                sy = y;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            Segment::Line(x, y) => {
                include(cx, cy);
                include(x, y);
                cx = x;
                cy = y;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            Segment::HLine(x) => {
                include(cx, cy);
                include(x, cy);
                cx = x;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            Segment::VLine(y) => {
                include(cx, cy);
                include(cx, y);
                cy = y;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            Segment::Cubic(x1, y1, x2, y2, x, y) => {
                include_cubic_bounds(&mut include, [(cx, cy), (x1, y1), (x2, y2), (x, y)]);
                cx = x;
                cy = y;
                last_cubic_ctrl = Some((x2, y2));
                last_quad_ctrl = None;
            }
            Segment::SmoothCubic(x2, y2, x, y) => {
                let (x1, y1) = last_cubic_ctrl
                    .map(|(px, py)| (cx + (cx - px), cy + (cy - py)))
                    .unwrap_or((cx, cy));
                include_cubic_bounds(&mut include, [(cx, cy), (x1, y1), (x2, y2), (x, y)]);
                cx = x;
                cy = y;
                last_cubic_ctrl = Some((x2, y2));
                last_quad_ctrl = None;
            }
            Segment::Quad(qx, qy, x, y) => {
                let (x1, y1, x2, y2) = quad_to_cubic(cx, cy, qx, qy, x, y);
                include_cubic_bounds(&mut include, [(cx, cy), (x1, y1), (x2, y2), (x, y)]);
                cx = x;
                cy = y;
                last_quad_ctrl = Some((qx, qy));
                last_cubic_ctrl = None;
            }
            Segment::SmoothQuad(x, y) => {
                let (qx, qy) = last_quad_ctrl
                    .map(|(px, py)| (cx + (cx - px), cy + (cy - py)))
                    .unwrap_or((cx, cy));
                let (x1, y1, x2, y2) = quad_to_cubic(cx, cy, qx, qy, x, y);
                include_cubic_bounds(&mut include, [(cx, cy), (x1, y1), (x2, y2), (x, y)]);
                cx = x;
                cy = y;
                last_quad_ctrl = Some((qx, qy));
                last_cubic_ctrl = None;
            }
            Segment::Arc(rx, ry, rotation, large, sweep, x, y) => {
                if (cx, cy) == (x, y) {
                    continue;
                }
                include(cx, cy);
                include(x, y);
                if rx != 0.0 && ry != 0.0 && (cx, cy) != (x, y) {
                    let arc = endpoint_arc(
                        (cx, cy),
                        (x, y),
                        rx,
                        ry,
                        rotation,
                        large != 0.0,
                        sweep != 0.0,
                    )?;
                    include_arc_bounds(&arc, &mut include);
                }
                cx = x;
                cy = y;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            Segment::Close => {
                cx = sx;
                cy = sy;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
        }
    }
    if ![min_x, min_y, max_x, max_y, max_x - min_x, max_y - min_y]
        .iter()
        .all(|v| v.is_finite())
    {
        return None;
    }
    Some(SvgPathBounds {
        x: min_x,
        y: min_y,
        w: max_x - min_x,
        h: max_y - min_y,
    })
}

fn include_cubic_bounds(include: &mut impl FnMut(f64, f64), points: [(f64, f64); 4]) {
    use kurbo::{CubicBez, Shape};
    let bounds = CubicBez::new(points[0], points[1], points[2], points[3]).bounding_box();
    include(bounds.x0, bounds.y0);
    include(bounds.x1, bounds.y1);
}

fn quad_to_cubic(x0: f64, y0: f64, qx: f64, qy: f64, x1: f64, y1: f64) -> (f64, f64, f64, f64) {
    (
        x0 / 3.0 + (2.0 / 3.0) * qx,
        y0 / 3.0 + (2.0 / 3.0) * qy,
        x1 / 3.0 + (2.0 / 3.0) * qx,
        y1 / 3.0 + (2.0 / 3.0) * qy,
    )
}

fn serialize_local_segments(segments: &[Segment], ox: f64, oy: f64) -> String {
    let mut out = String::new();
    for seg in segments {
        if !out.is_empty() {
            out.push(' ');
        }
        match *seg {
            Segment::Move(x, y) => write_pair(&mut out, 'M', x - ox, y - oy),
            Segment::Line(x, y) => write_pair(&mut out, 'L', x - ox, y - oy),
            Segment::HLine(x) => write_one(&mut out, 'H', x - ox),
            Segment::VLine(y) => write_one(&mut out, 'V', y - oy),
            Segment::Cubic(x1, y1, x2, y2, x, y) => {
                out.push('C');
                push_num(&mut out, x1 - ox);
                push_num(&mut out, y1 - oy);
                push_num(&mut out, x2 - ox);
                push_num(&mut out, y2 - oy);
                push_num(&mut out, x - ox);
                push_num(&mut out, y - oy);
            }
            Segment::SmoothCubic(x2, y2, x, y) => {
                out.push('S');
                push_num(&mut out, x2 - ox);
                push_num(&mut out, y2 - oy);
                push_num(&mut out, x - ox);
                push_num(&mut out, y - oy);
            }
            Segment::Quad(x1, y1, x, y) => {
                out.push('Q');
                push_num(&mut out, x1 - ox);
                push_num(&mut out, y1 - oy);
                push_num(&mut out, x - ox);
                push_num(&mut out, y - oy);
            }
            Segment::SmoothQuad(x, y) => write_pair(&mut out, 'T', x - ox, y - oy),
            Segment::Arc(rx, ry, rot, large, sweep, x, y) => {
                out.push('A');
                push_num(&mut out, rx);
                push_num(&mut out, ry);
                push_num(&mut out, rot);
                push_num(&mut out, large);
                push_num(&mut out, sweep);
                push_num(&mut out, x - ox);
                push_num(&mut out, y - oy);
            }
            Segment::Close => out.push('Z'),
        }
    }
    out
}

fn write_pair(out: &mut String, cmd: char, x: f64, y: f64) {
    out.push(cmd);
    push_num(out, x);
    push_num(out, y);
}

fn write_one(out: &mut String, cmd: char, n: f64) {
    out.push(cmd);
    push_num(out, n);
}

fn push_num(out: &mut String, n: f64) {
    out.push(' ');
    // Display emits the shortest decimal that round-trips this exact f64.
    // Do not snap small coordinates or truncate imported path precision.
    use std::fmt::Write;
    write!(out, "{n}").expect("writing SVG numbers to a String cannot fail");
}

#[cfg(test)]
mod tests {
    use super::localize_svg_path;

    #[test]
    fn preserves_arc_and_compound_moves() {
        let (d, bounds) = localize_svg_path("M10 50 A40 40 0 0 1 90 50 Z M5 5 H15").expect("path");
        assert!(d.contains('A'));
        assert_eq!(d.matches('M').count(), 2);
        assert_eq!(bounds.x, 5.0);
        assert_eq!(bounds.y, 5.0);
    }
}

/// Retain separate open/closed SVG contours without flattening curves or arcs.
/// Returned path data remains in the original source coordinate system.
pub(crate) fn split_closed_contours(data: &str) -> Option<(String, String)> {
    let segments = absolute_segments(data)?;
    let mut closed = Vec::new();
    let mut open = Vec::new();
    let mut contour = Vec::new();
    for segment in segments {
        if matches!(segment, Segment::Move(..)) && !contour.is_empty() {
            open.append(&mut contour);
        }
        let closes = matches!(segment, Segment::Close);
        contour.push(segment);
        if closes {
            closed.append(&mut contour);
        }
    }
    open.append(&mut contour);
    Some((
        serialize_local_segments(&closed, 0.0, 0.0),
        serialize_local_segments(&open, 0.0, 0.0),
    ))
}
