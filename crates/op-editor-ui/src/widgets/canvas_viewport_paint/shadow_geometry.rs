//! Shared silhouette geometry for outer-shadow painting and effect bounds.

use crate::Rect;

pub(super) fn spread_rect(rect: Rect, spread: f32) -> Option<Rect> {
    if !spread.is_finite() {
        return None;
    }
    if spread == 0.0 {
        return Some(rect);
    }
    let result = Rect::xywh(
        rect.origin.x - spread,
        rect.origin.y - spread,
        rect.size.x + spread * 2.0,
        rect.size.y + spread * 2.0,
    );
    let values = [
        result.origin.x,
        result.origin.y,
        result.size.x,
        result.size.y,
    ];
    (values.iter().all(|value| value.is_finite()) && result.size.x > 0.0 && result.size.y > 0.0)
        .then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RoundRectShadowOptions as Shadow;

    #[test]
    fn signed_spread_expands_or_contracts_the_silhouette_and_corner_radius() {
        let rect = Rect::xywh(10.0, 20.0, 100.0, 80.0);
        let outer = spread_rect(rect, 5.0).unwrap();
        assert_eq!(outer, Rect::xywh(5.0, 15.0, 110.0, 90.0));
        assert_eq!(Shadow::spread_radius(24.0, 5.0, outer), 29.0);
        let inner = spread_rect(rect, -5.0).unwrap();
        assert_eq!(inner, Rect::xywh(15.0, 25.0, 90.0, 70.0));
        assert_eq!(Shadow::spread_radius(24.0, -5.0, inner), 19.0);
    }

    #[test]
    fn zero_spread_is_an_exact_identity_and_collapsed_silhouettes_are_empty() {
        let rect = Rect::xywh(10.0, 20.0, 10.0, 8.0);
        assert_eq!(spread_rect(rect, 0.0), Some(rect));
        assert_eq!(Shadow::spread_radius(99.0, 0.0, rect), 99.0);
        assert_eq!(spread_rect(rect, -4.0), None);
        assert_eq!(spread_rect(rect, -20.0), None);
        assert_eq!(spread_rect(rect, f32::NAN), None);
    }

    #[test]
    fn contracted_corners_clamp_to_zero_and_square_corners_remain_square() {
        let rect = Rect::xywh(0.0, 0.0, 80.0, 60.0);
        assert_eq!(Shadow::spread_radius(3.0, -5.0, rect), 0.0);
        assert_eq!(Shadow::spread_radius(0.0, 5.0, rect), 0.0);
        assert_eq!(Shadow::spread_radius(50.0, 5.0, rect), 30.0);
    }
}
