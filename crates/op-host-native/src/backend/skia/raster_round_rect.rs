//! Polynomial rounded geometry for the point-camera export path.
//!
//! Circle clips retain their analytic contract. Ordinary rounded rectangles
//! instead have four cubic Bezier corners, as do editable polynomial paths.

use skia_safe::{Canvas, Paint, PaintStyle, PathBuilder, PathFillType, RRect, Vector};

pub(super) fn draw(canvas: &Canvas, rect: &RRect, paint: &Paint) -> bool {
    if !jian_skia::raster_sampling::is_point_sampling()
        || paint.mask_filter().is_some()
        || paint.image_filter().is_some()
        || !rect.rect().is_finite()
        || rect
            .radii_ref()
            .iter()
            .any(|r| !r.x.is_finite() || !r.y.is_finite())
    {
        return false;
    }
    let mut path = PathBuilder::new_with_fill_type(PathFillType::EvenOdd);
    match paint.style() {
        PaintStyle::Fill => append(&mut path, rect),
        PaintStyle::Stroke => {
            // Hairlines remain device-space platform strokes. Positive-width
            // borders are a single even-odd fill, avoiding a second blend at
            // the overlap between the four sides and their corner joins.
            let width = paint.stroke_width();
            if !width.is_finite() || width <= 0.0 {
                return false;
            }
            let half = width * 0.5;
            let outer = offset(rect, half);
            if !outer.rect().is_finite() {
                return false;
            }
            append(&mut path, &outer);
            if rect.rect().width() > width && rect.rect().height() > width {
                append(&mut path, &offset(rect, -half));
            }
        }
        _ => return false,
    }
    let mut source = paint.clone();
    source.set_style(PaintStyle::Fill);
    super::super::vector_text::draw_path(canvas, &path.detach(), &source)
}

fn offset(rect: &RRect, amount: f32) -> RRect {
    let radii = rect.radii_ref().map(|r| {
        if r.x == 0.0 || r.y == 0.0 {
            Vector::new(0.0, 0.0)
        } else {
            Vector::new((r.x + amount).max(0.0), (r.y + amount).max(0.0))
        }
    });
    RRect::new_rect_radii(rect.rect().with_outset((amount, amount)), &radii)
}

fn append(path: &mut PathBuilder, rrect: &RRect) {
    let rect = rrect.rect();
    let [tl, tr, br, bl] = *rrect.radii_ref();
    let [left, top, right, bottom] = [rect.left, rect.top, rect.right, rect.bottom].map(f64::from);
    let (tlx, tly) = (f64::from(tl.x), f64::from(tl.y));
    let (trx, try_) = (f64::from(tr.x), f64::from(tr.y));
    let (brx, bry) = (f64::from(br.x), f64::from(br.y));
    let (blx, bly) = (f64::from(bl.x), f64::from(bl.y));
    // Standard quarter-circle cubic. Round each completed local coordinate
    // once; rounding kappa before multiplying the radius loses source bits.
    let kappa = 4.0 * (2.0_f64.sqrt() - 1.0) / 3.0;
    let p = |x: f64, y: f64| skia_safe::Point::new(x as f32, y as f32);
    path.move_to(p(left + tlx, top));
    path.line_to(p(right - trx, top));
    path.cubic_to(
        p(right - trx + trx * kappa, top),
        p(right, top + try_ - try_ * kappa),
        p(right, top + try_),
    );
    path.line_to(p(right, bottom - bry));
    path.cubic_to(
        p(right, bottom - bry + bry * kappa),
        p(right - brx + brx * kappa, bottom),
        p(right - brx, bottom),
    );
    path.line_to(p(left + blx, bottom));
    path.cubic_to(
        p(left + blx - blx * kappa, bottom),
        p(left, bottom - bly + bly * kappa),
        p(left, bottom - bly),
    );
    path.line_to(p(left, top + tly));
    path.cubic_to(
        p(left, top + tly - tly * kappa),
        p(left + tlx - tlx * kappa, top),
        p(left + tlx, top),
    );
    path.close();
}

#[cfg(test)]
mod tests {
    use super::*;
    use skia_safe::{PathVerb, Point, Rect};

    #[test]
    fn rounded_polynomial_corners_keep_complete_coordinate_precision() {
        let rect = RRect::new_rect_xy(Rect::from_wh(37.0, 29.0), 4.0, 4.0);
        let mut path = PathBuilder::new();
        append(&mut path, &rect);
        let path = path.detach();
        let curves = path
            .iter()
            .filter(|r| r.verb() == PathVerb::Cubic)
            .collect::<Vec<_>>();
        assert_eq!(curves.len(), 4);
        let kappa = 4.0 * (2.0_f64.sqrt() - 1.0) / 3.0;
        assert_eq!(
            curves[0].points()[1],
            Point::new((33.0 + 4.0 * kappa) as f32, 0.0)
        );
        assert_eq!(
            curves[0].points()[2],
            Point::new(37.0, (4.0 - 4.0 * kappa) as f32)
        );
        assert_ne!((4.0 - 4.0 * kappa) as f32, 4.0 - 4.0 * kappa as f32);
    }

    #[test]
    fn rounded_border_offsets_keep_square_corners_and_normalize_small_radii() {
        let vectors = [
            Vector::new(0.0, 0.0),
            Vector::new(4.0, 3.0),
            Vector::new(2.0, 2.0),
            Vector::new(1.0, 1.0),
        ];
        let rect = RRect::new_rect_radii(Rect::from_wh(37.0, 29.0), &vectors);
        let outer = offset(&rect, 1.0);
        assert_eq!(outer.rect(), &Rect::from_xywh(-1.0, -1.0, 39.0, 31.0));
        assert_eq!(outer.radii_ref()[0], Vector::new(0.0, 0.0));
        assert_eq!(outer.radii_ref()[1], Vector::new(5.0, 4.0));
        let inner = offset(&rect, -2.0);
        assert_eq!(inner.radii_ref()[2], Vector::new(0.0, 0.0));
        assert_eq!(inner.radii_ref()[3], Vector::new(0.0, 0.0));
    }
}
