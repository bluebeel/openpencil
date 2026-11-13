//! Authored FILL rotation fitted using the selected decoded source dimensions.
use op_editor_ui::{ImageDrawMode, ImagePaintOptions, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceFitError {
    InvalidDimensions,
    NonFiniteRotation,
    DegenerateSourceSpan,
    NonFiniteTransform,
}

/// Row-major normalized UV affine [m00,m01,m02,m10,m11,m12]. Keep each
/// multiply/divide in f32: normalizing the source rectangle first loses the
/// source-size rounding that determines point-sampling boundary decisions.
pub(super) fn source_fit(
    source: [f32; 2],
    destination: [f32; 2],
    degrees: f32,
) -> Result<[f32; 6], SourceFitError> {
    if !source
        .into_iter()
        .chain(destination)
        .all(|v| v.is_finite() && v > 0.0)
    {
        return Err(SourceFitError::InvalidDimensions);
    }
    if !degrees.is_finite() {
        return Err(SourceFitError::NonFiniteRotation);
    }
    let theta = (f64::from(degrees) * std::f64::consts::PI / 180.0) as f32;
    let (sin, cos) = theta.sin_cos();
    let width = (cos * source[0] - sin * source[1]).abs();
    let height = (sin * source[0] + cos * source[1]).abs();
    if width == 0.0 || height == 0.0 {
        return Err(SourceFitError::DegenerateSourceSpan);
    }
    let sx = destination[0] / width;
    let sy = destination[1] / height;
    let maximum = sx.max(sy);
    let x = sx / maximum;
    let y = sy / maximum;
    let a = cos * x;
    let b = sin * y;
    let c = -sin * x;
    let d = cos * y;
    let matrix = [
        a,
        b,
        0.5 - (a * 0.5 + b * 0.5),
        c,
        d,
        0.5 - (c * 0.5 + d * 0.5),
    ];
    matrix
        .iter()
        .all(|v| v.is_finite())
        .then_some(matrix)
        .ok_or(SourceFitError::NonFiniteTransform)
}

/// Legacy paints, zero rotation, other fitting modes, and an explicit crop
/// affine keep their previous path. Complete authored turns are not zero.
pub(super) fn for_paint(
    rect: Rect,
    source: [f32; 2],
    options: &ImagePaintOptions<'_>,
) -> Option<Result<[f32; 6], SourceFitError>> {
    let degrees = options.rotation_degrees?;
    if degrees == 0.0 || options.mode != ImageDrawMode::Fill || options.transform.is_some() {
        return None;
    }
    Some(source_fit(source, [rect.size.x, rect.size.y], degrees))
}

pub(super) fn supported(
    rect: Rect,
    source: [f32; 2],
    options: &ImagePaintOptions<'_>,
) -> Option<[f32; 6]> {
    match for_paint(rect, source, options)? {
        Ok(matrix) => Some(matrix),
        Err(_) => {
            jian_skia::raster_sampling::mark_unsupported("invalid rotated FILL source span");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_source_dimensions_reproduce_observed_complete_and_quarter_turns() {
        assert_eq!(
            source_fit([375.0, 375.0], [64.0, 64.0], 360.0).unwrap(),
            [
                1.0,
                1.7484549630353285e-7,
                -5.960464477539063e-8,
                -1.7484555314695172e-7,
                0.9999996423721313,
                2.682209014892578e-7,
            ]
        );
        assert_eq!(
            source_fit([375.0, 375.0], [64.0, 64.0], 90.0).unwrap(),
            [
                -4.371138118131057e-8,
                1.0,
                2.9802322387695312e-8,
                -0.9999998211860657,
                -4.371138828673793e-8,
                0.9999999403953552,
            ]
        );
        assert_ne!(
            source_fit([64.0, 64.0], [64.0, 64.0], 90.0).unwrap(),
            source_fit([375.0, 375.0], [64.0, 64.0], 90.0).unwrap()
        );
    }

    #[test]
    fn rotation_policy_preserves_legacy_zero_crop_and_other_modes() {
        let rect = Rect::xywh(4.0, 8.0, 64.0, 64.0);
        let mut options = ImagePaintOptions::default();
        assert!(for_paint(rect, [375.0, 375.0], &options).is_none());
        options.rotation_degrees = Some(0.0);
        assert!(for_paint(rect, [375.0, 375.0], &options).is_none());
        options.rotation_degrees = Some(360.0);
        assert!(for_paint(rect, [375.0, 375.0], &options).unwrap().is_ok());
        options.transform = Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        assert!(for_paint(rect, [375.0, 375.0], &options).is_none());
        options.transform = None;
        for mode in [
            ImageDrawMode::Fit,
            ImageDrawMode::Crop,
            ImageDrawMode::Stretch,
            ImageDrawMode::Tile,
        ] {
            options.mode = mode;
            assert!(for_paint(rect, [375.0, 375.0], &options).is_none());
        }
    }

    #[test]
    fn invalid_rotation_and_degenerate_authored_source_spans_are_typed() {
        assert_eq!(
            source_fit([375.0, 375.0], [64.0, 64.0], 45.0),
            Err(SourceFitError::DegenerateSourceSpan)
        );
        assert_eq!(
            source_fit([0.0, 375.0], [64.0, 64.0], 0.0),
            Err(SourceFitError::InvalidDimensions)
        );
        assert_eq!(
            source_fit([375.0, 375.0], [64.0, 64.0], f32::NAN),
            Err(SourceFitError::NonFiniteRotation)
        );
    }
}
