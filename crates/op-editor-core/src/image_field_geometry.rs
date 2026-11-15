//! Shared f64 setup for explicit source, image viewport and tile coordinates.
use jian_ops_schema::style::{ImageExtend, ImageFieldGeometry, ImageFieldUnits, ImageViewportFit};
use kurbo::{Affine, Point};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFieldError {
    InvalidBounds,
    NonFinite,
    Singular,
    MissingIntrinsicDimensions,
    InfinitePatternOverlap,
}
impl std::fmt::Display for ImageFieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidBounds => "invalid image paint bounds",
            Self::NonFinite => "non-finite image field transform",
            Self::Singular => "singular image field transform",
            Self::MissingIntrinsicDimensions => {
                "image field is awaiting intrinsic source dimensions"
            }
            Self::InfinitePatternOverlap => {
                "unclipped repeated image fields require finite source support"
            }
        })
    }
}
impl std::error::Error for ImageFieldError {}
#[derive(Debug, Clone)]
pub struct PreparedImageField {
    pub image_to_document: Affine,
    pub viewport_to_document: Affine,
    pub tile_to_document: Affine,
    pub document_to_tile: Affine,
    pub tile_source_bounds: [f64; 4],
    pub source_extend: [ImageExtend; 2],
    pub pattern_extend: [ImageExtend; 2],
    pub patterned: bool,
    pub clip_tile: bool,
    pub clip_viewport: bool,
}
fn finite(m: Affine) -> Result<Affine, ImageFieldError> {
    if m.as_coeffs().iter().all(|n| n.is_finite()) {
        Ok(m)
    } else {
        Err(ImageFieldError::NonFinite)
    }
}
fn inverse(m: Affine) -> Result<Affine, ImageFieldError> {
    if m.determinant() == 0.0 {
        return Err(ImageFieldError::Singular);
    }
    finite(m.inverse())
}
fn corners(m: Affine) -> Vec<Point> {
    [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
        .map(|p| m * Point::new(p.0, p.1))
        .to_vec()
}
pub fn prepare(
    geometry: &ImageFieldGeometry,
    bounds: [f64; 4],
) -> Result<PreparedImageField, ImageFieldError> {
    prepare_with_source(geometry, bounds, [0, 0])
}
pub fn prepare_with_source(
    geometry: &ImageFieldGeometry,
    bounds: [f64; 4],
    dimensions: [u32; 2],
) -> Result<PreparedImageField, ImageFieldError> {
    let [x, y, w, h] = bounds;
    if !bounds.iter().all(|v| v.is_finite()) || w <= 0.0 || h <= 0.0 {
        return Err(ImageFieldError::InvalidBounds);
    }
    let basis = Affine::translate((x, y)) * Affine::scale_non_uniform(w, h);
    let mut placement = finite(Affine::new(geometry.transform))?;
    inverse(placement)?;
    let mut viewport_size = dimensions.map(f64::from);
    let (mut fit, mut clip_viewport) = (Affine::IDENTITY, false);
    if let Some(viewport) = &geometry.viewport {
        let [mut vw, mut vh] = viewport.size;
        let [iw, ih] = dimensions.map(f64::from);
        if vw == 0.0 && vh == 0.0 {
            vw = iw;
            vh = ih;
        } else if vw == 0.0 && ih > 0.0 {
            vw = vh * iw / ih;
        } else if vh == 0.0 && iw > 0.0 {
            vh = vw * ih / iw;
        }
        viewport_size = [vw, vh];
        if ![vw, vh, viewport.align[0], viewport.align[1]]
            .iter()
            .all(|v| v.is_finite())
            || vw <= 0.0
            || vh <= 0.0
        {
            return Err(ImageFieldError::InvalidBounds);
        }
        clip_viewport = viewport.clip;
        if viewport.fit != ImageViewportFit::None {
            let [iw, ih] = dimensions.map(f64::from);
            if iw <= 0.0 || ih <= 0.0 {
                return Err(ImageFieldError::MissingIntrinsicDimensions);
            }
            let s = if viewport.fit == ImageViewportFit::Meet {
                (vw / iw).min(vh / ih)
            } else {
                (vw / iw).max(vh / ih)
            };
            let (dw, dh) = (iw * s / vw, ih * s / vh);
            fit = Affine::translate((
                (1.0 - dw) * viewport.align[0],
                (1.0 - dh) * viewport.align[1],
            )) * Affine::scale_non_uniform(dw, dh);
        }
    }
    if geometry.units != ImageFieldUnits::Normalized {
        if viewport_size.iter().any(|v| *v <= 0.0) {
            return Err(ImageFieldError::MissingIntrinsicDimensions);
        }
        let d = Affine::scale_non_uniform(viewport_size[0], viewport_size[1]);
        placement = finite(if geometry.units == ImageFieldUnits::UserSpace {
            placement * d
        } else {
            d.inverse() * placement * d
        })?;
    }
    let image = finite(placement * fit)?;
    let (patterned, clip_tile, tile, pattern_extend) = if let Some(pattern) = &geometry.pattern {
        if !pattern.clip
            && (geometry.extend_x != ImageExtend::Decal || geometry.extend_y != ImageExtend::Decal)
        {
            return Err(ImageFieldError::InfinitePatternOverlap);
        }
        (
            true,
            pattern.clip,
            finite(basis * Affine::new(pattern.transform))?,
            [pattern.extend_x.extension(), pattern.extend_y.extension()],
        )
    } else {
        (false, true, basis, [ImageExtend::Decal; 2])
    };
    let mut support = corners(image);
    if clip_viewport {
        support = clip_unit(support, inverse(placement)?, [true, true]);
    }
    let tile_source_bounds = polygon_bounds(&support).unwrap_or([0.0; 4]);
    Ok(PreparedImageField {
        image_to_document: finite(tile * image)?,
        viewport_to_document: finite(tile * placement)?,
        tile_to_document: tile,
        document_to_tile: inverse(tile)?,
        tile_source_bounds,
        source_extend: [geometry.extend_x, geometry.extend_y],
        pattern_extend,
        patterned,
        clip_tile,
        clip_viewport,
    })
}
/// Clip a finite paint coverage polygon against the source/viewport support.
/// Curved shape clipping remains in the caller; these are only affine edges.
pub fn visible_polygon(
    setup: &PreparedImageField,
    coverage: [f64; 4],
) -> Result<Vec<[f64; 2]>, ImageFieldError> {
    let [x, y, w, h] = coverage;
    let mut polygon = vec![
        Point::new(x, y),
        Point::new(x + w, y),
        Point::new(x + w, y + h),
        Point::new(x, y + h),
    ];
    polygon = clip_unit(
        polygon,
        inverse(setup.image_to_document)?,
        setup.source_extend.map(|e| e == ImageExtend::Decal),
    );
    if setup.clip_viewport {
        polygon = clip_unit(polygon, inverse(setup.viewport_to_document)?, [true, true]);
    }
    Ok(polygon.into_iter().map(|p| [p.x, p.y]).collect())
}
pub fn source_bounds(setup: &PreparedImageField) -> Option<[f64; 4]> {
    let mut polygon = corners(setup.image_to_document);
    if setup.clip_viewport {
        polygon = clip_unit(polygon, setup.viewport_to_document.inverse(), [true, true]);
    }
    polygon_bounds(&polygon).map(|[x, y, r, b]| [x, y, r - x, b - y])
}
fn polygon_bounds(points: &[Point]) -> Option<[f64; 4]> {
    if points.is_empty() {
        return None;
    }
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for p in points {
        b[0] = b[0].min(p.x);
        b[1] = b[1].min(p.y);
        b[2] = b[2].max(p.x);
        b[3] = b[3].max(p.y);
    }
    Some(b)
}
fn clip_unit(mut points: Vec<Point>, inverse: Affine, axes: [bool; 2]) -> Vec<Point> {
    for axis in 0..2 {
        if !axes[axis] {
            continue;
        }
        for edge in 0..2 {
            if points.is_empty() {
                return points;
            }
            let coordinate = |p: Point| {
                let p = inverse * p;
                if axis == 0 {
                    p.x
                } else {
                    p.y
                }
            };
            let inside = |v: f64| if edge == 0 { v >= 0.0 } else { v <= 1.0 };
            let mut output = Vec::with_capacity(points.len() + 1);
            let mut previous = *points.last().unwrap();
            let mut pv = coordinate(previous);
            for current in points {
                let cv = coordinate(current);
                let (pi, ci) = (inside(pv), inside(cv));
                if pi != ci {
                    let t = (edge as f64 - pv) / (cv - pv);
                    output.push(previous + (current - previous) * t);
                }
                if ci {
                    output.push(current);
                }
                previous = current;
                pv = cv;
            }
            points = output;
        }
    }
    points
}
/// Rebase a field after SVG transforms are baked into a new node bounding box.
pub fn rebase(
    geometry: &ImageFieldGeometry,
    old_bounds: [f64; 4],
    matrix: Affine,
    new_bounds: [f64; 4],
) -> Result<ImageFieldGeometry, ImageFieldError> {
    let [x, y, w, h] = old_bounds;
    let [nx, ny, nw, nh] = new_bounds;
    if nw <= 0.0 || nh <= 0.0 {
        return Err(ImageFieldError::InvalidBounds);
    }
    let rebased = finite(
        Affine::scale_non_uniform(1.0 / nw, 1.0 / nh)
            * Affine::translate((-nx, -ny))
            * matrix
            * Affine::translate((x, y))
            * Affine::scale_non_uniform(w, h),
    )?;
    let mut output = geometry.clone();
    if let Some(pattern) = &mut output.pattern {
        pattern.transform = finite(rebased * Affine::new(pattern.transform))?.as_coeffs();
    } else {
        output.transform = finite(rebased * Affine::new(output.transform))?.as_coeffs();
    }
    Ok(output)
}

pub const PATTERN_SHADER: &str = include_str!("image_field_geometry/pattern.sksl");

pub const SOURCE_SHADER: &str = include_str!("image_field_geometry/source.sksl");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFitError {
    InvalidDimensions,
    NonFiniteRotation,
    DegenerateSourceSpan,
    NonFiniteTransform,
}

/// Row-major normalized UV affine [m00,m01,m02,m10,m11,m12]. Keep each
/// multiply/divide in f32: normalizing the source rectangle first loses the
/// source-size rounding that determines point-sampling boundary decisions.
pub fn source_fit(
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

pub fn image_adjustment_matrix(
    adjustments: crate::render_backend::ImageAdjustments,
) -> Option<[f32; 20]> {
    let exp = adjustments.exposure / 100.0;
    let con = adjustments.contrast / 100.0;
    let sat = adjustments.saturation / 100.0;
    let temp = adjustments.temperature / 100.0;
    let tint = adjustments.tint / 100.0;
    let hi = adjustments.highlights / 100.0;
    let sh = adjustments.shadows / 100.0;
    if adjustments.is_neutral() {
        return None;
    }

    let e = 1.0 + exp * 1.5;
    let c = 1.0 + con;
    let c_off = 0.5 * (1.0 - c);
    let s = 1.0 + sat;
    let (lr, lg, lb) = (0.2126, 0.7152, 0.0722);
    let sr = (1.0 - s) * lr;
    let sg = (1.0 - s) * lg;
    let sb = (1.0 - s) * lb;
    let f = c * e;
    let off_r = c_off + temp * 0.15 + (hi + sh * 0.5) * 0.1;
    let off_g = c_off + tint * 0.15 + (hi + sh * 0.5) * 0.1;
    let off_b = c_off - temp * 0.15 + (hi + sh * 0.5) * 0.1;

    Some([
        f * (sr + s),
        f * sg,
        f * sb,
        0.0,
        off_r,
        f * sr,
        f * (sg + s),
        f * sb,
        0.0,
        off_g,
        f * sr,
        f * sg,
        f * (sb + s),
        0.0,
        off_b,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
    ])
}
