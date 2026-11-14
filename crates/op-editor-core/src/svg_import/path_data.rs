//! SVG commands to editable f64 anchors. Preserved-d imports keep their exact
//! arc primitives; this editable representation approximates arcs with cubics.

use super::*;
use crate::svg_path_data::{endpoint_arc, AbsoluteSegments, Segment};

/// Read each command once, with SVG's implicit repetition, relative coordinates
/// and single-character arc flags handled by the shared grammar reader.
/// A malformed tail retains the valid prefix, matching this legacy helper.
pub(super) fn parse_path_d(d: &str, offset: (f64, f64)) -> Vec<(Vec<PenPathAnchor>, bool)> {
    if !offset.0.is_finite() || !offset.1.is_finite() {
        return Vec::new();
    }
    let mut subpaths = Vec::new();
    let mut anchors = Vec::new();
    let mut current = (0.0, 0.0);
    let mut start = current;
    let mut cubic_control = None;
    let mut quad_control = None;
    for segment in AbsoluteSegments::new(d) {
        let Ok(segment) = segment else {
            break;
        };
        if let Segment::Move(x, y) = segment {
            finish_subpath(&mut subpaths, &mut anchors, false);
            current = (x, y);
            start = current;
            push_anchor(&mut anchors, current, offset);
            cubic_control = None;
            quad_control = None;
            continue;
        }
        if let Segment::Close = segment {
            finish_subpath(&mut subpaths, &mut anchors, true);
            current = start;
            cubic_control = None;
            quad_control = None;
            continue;
        }
        // A drawing command after closepath starts at the closed contour's
        // initial point; it must not extend that already-closed anchor list.
        if anchors.is_empty() {
            start = current;
            push_anchor(&mut anchors, current, offset);
        }
        let previous_cubic = cubic_control.take();
        let previous_quad = quad_control.take();
        match segment {
            Segment::Line(x, y) => {
                current = (x, y);
                push_anchor(&mut anchors, current, offset);
            }
            Segment::HLine(x) => {
                current.0 = x;
                push_anchor(&mut anchors, current, offset);
            }
            Segment::VLine(y) => {
                current.1 = y;
                push_anchor(&mut anchors, current, offset);
            }
            Segment::Cubic(x1, y1, x2, y2, x, y) => {
                emit_cubic(&mut anchors, (x1, y1), (x2, y2), (x, y), offset);
                current = (x, y);
                cubic_control = Some((x2, y2));
            }
            Segment::SmoothCubic(x2, y2, x, y) => {
                let c1 = reflect(previous_cubic, current);
                emit_cubic(&mut anchors, c1, (x2, y2), (x, y), offset);
                current = (x, y);
                cubic_control = Some((x2, y2));
            }
            Segment::Quad(qx, qy, x, y) => {
                emit_quad(&mut anchors, current, (qx, qy), (x, y), offset);
                current = (x, y);
                quad_control = Some((qx, qy));
            }
            Segment::SmoothQuad(x, y) => {
                let control = reflect(previous_quad, current);
                emit_quad(&mut anchors, current, control, (x, y), offset);
                current = (x, y);
                quad_control = Some(control);
            }
            Segment::Arc(rx, ry, rotation, large, sweep, x, y) => {
                let end = (x, y);
                if current == end {
                    // SVG omits a coincident-endpoint arc, even for large-arc.
                } else if rx == 0.0 || ry == 0.0 {
                    push_anchor(&mut anchors, end, offset);
                } else {
                    let Some(arc) =
                        endpoint_arc(current, end, rx, ry, rotation, large != 0.0, sweep != 0.0)
                    else {
                        break;
                    };
                    // Kurbo performs curve subdivision, not polygon flattening.
                    // Relative tolerance also bounds subdivision count for huge
                    // finite radii; exact circular/conic primitives stay in d.
                    let tolerance = (arc.radii.x.max(arc.radii.y) * 1e-9).max(1e-7);
                    let mut curves = arc.append_iter(tolerance).peekable();
                    while let Some(kurbo::PathEl::CurveTo(c1, c2, p)) = curves.next() {
                        // Keep the authored endpoint exactly, rather than the
                        // last trigonometric evaluation's floating-point drift.
                        let p = if curves.peek().is_none() {
                            end
                        } else {
                            (p.x, p.y)
                        };
                        emit_cubic(&mut anchors, (c1.x, c1.y), (c2.x, c2.y), p, offset);
                    }
                }
                current = end;
            }
            Segment::Move(..) | Segment::Close => unreachable!("handled before segment drawing"),
        }
    }
    finish_subpath(&mut subpaths, &mut anchors, false);
    subpaths
}

fn finish_subpath(
    subpaths: &mut Vec<(Vec<PenPathAnchor>, bool)>,
    anchors: &mut Vec<PenPathAnchor>,
    closed: bool,
) {
    // One logical seam anchor owns both the arriving and departing handles.
    // Keep a two-anchor closed loop: a single anchor is not supported by the
    // existing editable-path constructor.
    if closed && anchors.len() > 2 {
        let first = &anchors[0];
        let last = anchors.last().expect("nonempty path");
        if first.x == last.x && first.y == last.y {
            let last = anchors.pop().expect("duplicate seam anchor");
            anchors[0].handle_in = last.handle_in;
        }
    }
    if anchors.len() >= 2 {
        subpaths.push((std::mem::take(anchors), closed));
    } else {
        anchors.clear();
    }
}

fn push_anchor(anchors: &mut Vec<PenPathAnchor>, point: (f64, f64), offset: (f64, f64)) {
    anchors.push(PenPathAnchor {
        x: point.0 + offset.0,
        y: point.1 + offset.1,
        handle_in: None,
        handle_out: None,
        point_type: None,
    });
}

fn reflect(previous: Option<(f64, f64)>, current: (f64, f64)) -> (f64, f64) {
    previous
        .map(|p| (current.0 + (current.0 - p.0), current.1 + (current.1 - p.1)))
        .unwrap_or(current)
}

fn emit_quad(
    anchors: &mut Vec<PenPathAnchor>,
    start: (f64, f64),
    control: (f64, f64),
    end: (f64, f64),
    offset: (f64, f64),
) {
    let c1 = (
        start.0 / 3.0 + control.0 * (2.0 / 3.0),
        start.1 / 3.0 + control.1 * (2.0 / 3.0),
    );
    let c2 = (
        end.0 / 3.0 + control.0 * (2.0 / 3.0),
        end.1 / 3.0 + control.1 * (2.0 / 3.0),
    );
    emit_cubic(anchors, c1, c2, end, offset);
}

/// Store controls as anchor-relative deltas, preserving editable handles.
fn emit_cubic(
    anchors: &mut Vec<PenPathAnchor>,
    c1: (f64, f64),
    c2: (f64, f64),
    end: (f64, f64),
    offset: (f64, f64),
) {
    let Some(previous) = anchors.last_mut() else {
        return;
    };
    previous.handle_out = Some(PenPathHandle {
        x: c1.0 + offset.0 - previous.x,
        y: c1.1 + offset.1 - previous.y,
    });
    anchors.push(PenPathAnchor {
        x: end.0 + offset.0,
        y: end.1 + offset.1,
        handle_in: Some(PenPathHandle {
            x: c2.0 - end.0,
            y: c2.1 - end.1,
        }),
        handle_out: None,
        point_type: None,
    });
}
