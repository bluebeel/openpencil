//! Lexical output support, transformed into each text field's local space.
//! It never changes document geometry, root viewBox, clipping or paint mapping.
use super::super::*;
use std::cell::Cell;
#[derive(Clone, Copy)]
struct Frame {
    output: Rect,
    current: glam::Affine2,
    parent: glam::Affine2,
}
thread_local! { static CURRENT: Cell<Option<Frame>> = const { Cell::new(None) }; }
pub(in crate::svg_export) struct Scope(Option<Frame>);
impl Scope {
    fn replace(value: Option<Frame>) -> Self {
        Self(CURRENT.with(|s| s.replace(value)))
    }
    pub(in crate::svg_export) fn root(output: Rect, current: glam::Affine2) -> Self {
        Self::replace(Some(Frame {
            output,
            current,
            parent: current,
        }))
    }
    pub(in crate::svg_export) fn node(node: &SceneNode) -> Self {
        Self::replace(CURRENT.with(|s| s.get()).map(|mut f| {
            f.parent = f.current;
            f.current = f.current * node_transform(node);
            f
        }))
    }
    pub(in crate::svg_export) fn world(parent: glam::Affine2) -> Self {
        Self::replace(CURRENT.with(|s| s.get()).map(|mut f| {
            f.current = parent;
            f.parent = parent;
            f
        }))
    }
    pub(in crate::svg_export) fn mask_parent(parent: glam::Affine2, ancestor: bool) -> Self {
        Self::replace(CURRENT.with(|s| s.get()).map(|mut f| {
            f.current = if ancestor { parent } else { f.parent * parent };
            f.parent = f.current;
            f
        }))
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|s| s.set(self.0));
    }
}
pub(super) fn area() -> Option<Rect> {
    let f = CURRENT.with(|s| s.get())?;
    let [a, b, c, d, tx, ty] = f.current.to_cols_array().map(f64::from);
    let det = a * d - b * c;
    if !det.is_finite() || det == 0.0 {
        return None;
    }
    let r = f.output;
    let (x, y, w, h) = (
        f64::from(r.origin.x),
        f64::from(r.origin.y),
        f64::from(r.size.x),
        f64::from(r.size.y),
    );
    let (mut left, mut top, mut right, mut bottom) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for (x, y) in [(x, y), (x + w, y), (x, y + h), (x + w, y + h)] {
        let (x, y) = (x - tx, y - ty);
        let (x, y) = ((d * x - c * y) / det, (a * y - b * x) / det);
        left = left.min(x);
        right = right.max(x);
        top = top.min(y);
        bottom = bottom.max(y);
    }
    // Outward float32 endpoints cover inverse/storage rounding; they are only
    // allocation/field coverage, never a text placement or fitting epsilon.
    let (left, top, right, bottom) = (
        (left as f32).next_down(),
        (top as f32).next_down(),
        (right as f32).next_up(),
        (bottom as f32).next_up(),
    );
    let r = Rect::xywh(
        left,
        top,
        (right - left).next_up(),
        (bottom - top).next_up(),
    );
    [r.origin.x, r.origin.y, r.size.x, r.size.y]
        .into_iter()
        .all(f32::is_finite)
        .then_some(r)
        .filter(|r| r.size.x > 0.0 && r.size.y > 0.0)
}
