//! Point-sampled quadratic contours without polyline subdivision.
//!
//! Quadratics are degree-elevated to cubics and their control hulls use the
//! standard implicit equation k^3 - l*m. This is the Loop--Blinn construction:
//! https://www.microsoft.com/en-us/research/wp-content/uploads/2005/01/p1000-loop.pdf
//! Triangle setup retains the imported renderer's Float32 interpolation stages.
//! Original paint, clipping and backdrop compositing remain with the caller.

use skia_safe::{
    AlphaType, ColorType, Data, IRect, ImageInfo, Matrix, Path, PathFillType, PathVerb, Point,
    Rect, SamplingOptions, Shader, TileMode,
};

#[path = "vector_text_mask_cache.rs"]
mod mask_cache;

pub(super) fn trim_retained_cache(budget: usize) {
    mask_cache::trim(budget);
}
#[cfg(test)]
pub(super) fn retained_coverage_bytes() -> usize {
    mask_cache::retained_bytes()
}

const MAX_TRIANGLES: usize = 65536;
const MAX_WORKING_BYTES: usize = 32 * 1024 * 1024;
const MAX_DEVICE_COORDINATE: f32 = 1_000_000.0;
const JITTERS: [(f32, f32); 4] = [
    (0.625, 0.875),
    (0.375, 0.125),
    (0.125, 0.625),
    (0.875, 0.375),
];
const INTERIOR: [f32; 3] = [0.5, 0.5, 0.5];
const CUBIC: [[f32; 3]; 4] = [
    [0.0, 0.0, 0.0],
    [1.0 / 3.0, 0.0, 1.0 / 3.0],
    [2.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0],
    [1.0, 1.0, 1.0],
];

#[derive(Clone, Copy)]
struct Triangle {
    points: [Point; 3],
    values: [[f32; 3]; 3],
    interior: bool,
}

fn append(
    triangles: &mut Vec<Triangle>,
    points: [Point; 3],
    values: [[f32; 3]; 3],
    interior: bool,
) -> Option<()> {
    if triangles.len() == MAX_TRIANGLES {
        return None;
    }
    triangles.try_reserve(1).ok()?;
    triangles.push(Triangle {
        points,
        values,
        interior,
    });
    Some(())
}

fn elevated(a: Point, control: Point) -> Point {
    Point::new(
        ((f64::from(a.x) + 2.0 * f64::from(control.x)) / 3.0) as f32,
        ((f64::from(a.y) + 2.0 * f64::from(control.y)) / 3.0) as f32,
    )
}

fn triangles(path: &Path) -> Option<Vec<Triangle>> {
    if path.is_inverse_fill_type() || path.count_points() > MAX_TRIANGLES {
        return None;
    }
    let mut result = Vec::new();
    let (mut first, mut previous) = (None, None);
    for record in path.iter() {
        let p = record.points();
        if p.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
            return None;
        }
        match record.verb() {
            PathVerb::Move => {
                first = Some(p[0]);
                previous = Some(p[0]);
            }
            PathVerb::Line => {
                append(&mut result, [first?, previous?, p[1]], [INTERIOR; 3], true)?;
                previous = Some(p[1]);
            }
            PathVerb::Quad => {
                append(&mut result, [first?, previous?, p[2]], [INTERIOR; 3], true)?;
                let a = elevated(p[0], p[1]);
                let b = elevated(p[2], p[1]);
                append(
                    &mut result,
                    [p[0], a, p[2]],
                    [CUBIC[0], CUBIC[1], CUBIC[3]],
                    false,
                )?;
                append(
                    &mut result,
                    [a, b, p[2]],
                    [CUBIC[1], CUBIC[2], CUBIC[3]],
                    false,
                )?;
                previous = Some(p[2]);
            }
            PathVerb::Close => {
                first = None;
                previous = None;
            }
            // Cubic and rational outlines retain the established path fallback.
            PathVerb::Cubic | PathVerb::Conic => return None,
        }
    }
    Some(result)
}

#[derive(Clone, Copy)]
struct Plane {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    origin: [f32; 2],
}

fn plane(points: [[i64; 2]; 3], values: [[f32; 3]; 3], screen: [Point; 3]) -> Option<Plane> {
    // Last-provoking assembly, followed by minimum unflipped-Y rotations.
    // Both rotations run for equal minima; changing the anchor changes ties.
    let mut order = [2, 0, 1];
    let y = order.map(|i| screen[i].y);
    let minimum = y.into_iter().fold(f32::INFINITY, f32::min);
    if minimum == y[1] {
        order.rotate_left(1);
    }
    if minimum == y[2] {
        order.rotate_left(2);
    }
    let [i0, i1, i2] = order;
    let p = points.map(|p| [p[0] as f32 / 16.0, p[1] as f32 / 16.0]);
    let [x0, y0] = p[i0];
    let (x1, y1) = (p[i1][0] - x0, p[i1][1] - y0);
    let (x2, y2) = (p[i2][0] - x0, p[i2][1] - y0);
    let determinant = x1 * y2 - x2 * y1;
    if determinant == 0.0 || !determinant.is_finite() {
        return None;
    }
    let inverse = 1.0 / determinant;
    let m = [
        [(y1 - y2) * inverse, (x2 - x1) * inverse, 1.0],
        [y2 * inverse, -x2 * inverse, 0.0],
        [-y1 * inverse, x1 * inverse, 0.0],
    ];
    let mut coefficients = [[0.0; 4]; 3];
    for axis in 0..3 {
        for c in 0..3 {
            coefficients[axis][c] = (values[i0][c] * m[0][axis] + values[i1][c] * m[1][axis])
                + values[i2][c] * m[2][axis];
        }
        coefficients[axis][3] = (m[0][axis] + m[1][axis]) + m[2][axis];
    }
    coefficients
        .iter()
        .flatten()
        .all(|v| v.is_finite())
        .then_some(Plane {
            a: coefficients[0],
            b: coefficients[1],
            c: coefficients[2],
            origin: [x0, y0],
        })
}

fn cross(a: [i64; 2], b: [i64; 2], p: [i64; 2]) -> i64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

#[cfg(test)]
fn inside(points: [[i64; 2]; 3], point: [i64; 2], orientation: i64) -> bool {
    for i in 0..3 {
        let a = points[i];
        let b = points[(i + 1) % 3];
        let value = cross(a, b, point);
        if value == 0 {
            let dy = b[1] - a[1];
            let inclusive = if dy != 0 {
                dy.signum() * orientation.signum() < 0
            } else {
                points[(i + 2) % 3][1] > a[1]
            };
            if !inclusive {
                return false;
            }
        } else if (value > 0) != (orientation > 0) {
            return false;
        }
    }
    true
}

// Intersect three exact integer half-planes once per row. Constant interior
// triangles then add a contiguous stencil span; curved triangles evaluate only
// the implicit equation within that span, not three edge products per pixel.
fn span(
    points: [[i64; 2]; 3],
    y: i32,
    orientation: i64,
    mut left: i32,
    mut right: i32,
) -> Option<(i32, i32)> {
    let orientation = orientation.signum();
    let floor_div = |a: i64, b: i64| {
        if b > 0 {
            a.div_euclid(b)
        } else {
            (-a).div_euclid(-b)
        }
    };
    for i in 0..3 {
        let a = points[i];
        let b = points[(i + 1) % 3];
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let inclusive = if dy != 0 {
            dy.signum() * orientation < 0
        } else {
            points[(i + 2) % 3][1] > a[1]
        };
        let threshold = i64::from(!inclusive);
        let slope = -16 * dy * orientation;
        let intercept = (dx * (-i64::from(y) * 16 - a[1]) + dy * a[0]) * orientation;
        if slope > 0 {
            let start = -floor_div(intercept - threshold, slope);
            left = i64::from(left).max(start).min(i64::from(i32::MAX)) as i32;
        } else if slope < 0 {
            let end = floor_div(threshold - intercept, slope) + 1;
            right = i64::from(right).min(end).max(i64::from(i32::MIN)) as i32;
        } else if intercept < threshold {
            return None;
        }
        if left >= right {
            return None;
        }
    }
    Some((left, right))
}

fn keep(plane: Plane, x: i32, y: i32) -> bool {
    let dx = x as f32 - plane.origin[0];
    let dy = -(y as f32) - plane.origin[1];
    let mut value = [0.0; 4];
    for (i, value) in value.iter_mut().enumerate() {
        let row = plane.c[i] + dy * plane.b[i];
        *value = row + dx * plane.a[i];
    }
    let reciprocal_w = 1.0 / value[3];
    let k = value[0] * reciprocal_w;
    let l = value[1] * reciprocal_w;
    let m = value[2] * reciprocal_w;
    (k * k) * k <= l * m
}

fn map(matrix: &Matrix, point: Point) -> Point {
    Point::new(
        matrix.scale_x() * point.x + matrix.skew_x() * point.y + matrix.translate_x(),
        matrix.skew_y() * point.x + matrix.scale_y() * point.y + matrix.translate_y(),
    )
}

struct Camera<'a> {
    matrix: &'a Matrix,
    grid: i32,
    jitter: (f32, f32),
    point: bool,
}
struct Target<'a> {
    bounds: Rect,
    stencil: &'a mut [i32],
    width: usize,
    height: usize,
}

fn raster(triangle: Triangle, camera: &Camera<'_>, target: &mut Target<'_>) -> Option<()> {
    let Camera {
        matrix,
        grid,
        jitter,
        point,
    } = *camera;
    let bounds = target.bounds;
    let width = target.width;
    let height = target.height;
    let stencil = &mut target.stencil;
    let screen = triangle.points.map(|p| map(matrix, p));
    let projected = screen.map(|p| {
        if point {
            [p.x - 0.5, 0.5 - p.y]
        } else {
            [grid as f32 * p.x - jitter.0, -grid as f32 * p.y + jitter.1]
        }
    });
    if projected
        .iter()
        .flatten()
        .any(|v| !v.is_finite() || v.abs() > MAX_DEVICE_COORDINATE)
    {
        return None;
    }
    let p = projected.map(|p| p.map(|v| (v * 16.0).round_ties_even() as i64));
    let area = cross(p[0], p[1], p[2]);
    if area == 0 {
        return Some(());
    }
    let plane = if triangle.interior {
        None
    } else {
        Some(plane(p, triangle.values, screen)?)
    };
    let sign = if area > 0 { 1 } else { -1 };
    let left = (p.iter().map(|p| p[0]).min()? as f64 / 16.0).ceil() as i32;
    let right = (p.iter().map(|p| p[0]).max()? as f64 / 16.0).floor() as i32 + 1;
    let top = (-p.iter().map(|p| p[1]).max()? as f64 / 16.0).ceil() as i32;
    let bottom = (-p.iter().map(|p| p[1]).min()? as f64 / 16.0).floor() as i32 + 1;
    let origin_x = bounds.left as i32 * grid;
    let origin_y = bounds.top as i32 * grid;
    let left = left.max(origin_x);
    let right = right.min(origin_x + width as i32);
    for y in top.max(origin_y)..bottom.min(origin_y + height as i32) {
        let Some((start, end)) = span(p, y, area, left, right) else {
            continue;
        };
        let row = (y - origin_y) as usize * width;
        if let Some(plane) = plane {
            for x in start..end {
                if keep(plane, x, y) {
                    stencil[row + (x - origin_x) as usize] += sign;
                }
            }
        } else {
            for value in
                &mut stencil[row + (start - origin_x) as usize..row + (end - origin_x) as usize]
            {
                *value += sign;
            }
        }
    }
    Some(())
}

pub(super) fn coverage(
    path: &Path,
    matrix: &Matrix,
    clip: IRect,
    grid: u32,
) -> Option<(Shader, Rect)> {
    if !matches!(grid, 0..=2)
        || !matrix.is_finite()
        || matrix.has_perspective()
        || path.count_points() > MAX_TRIANGLES
    {
        return None;
    }
    let budget = super::reuse::cache_headroom();
    if budget == 0 {
        mask_cache::trim(0);
        return coverage_uncached(path, matrix, clip, grid, 0)
            .map(|(shader, bounds, _)| (shader, bounds));
    }
    let key = mask_cache::Key::new(path, matrix, clip, grid);
    if let Some(result) = mask_cache::lookup(&key, super::reuse::cache_headroom()) {
        return Some(result);
    }
    let Some((shader, bounds, bytes)) = coverage_uncached(path, matrix, clip, grid, key.bytes())
    else {
        // A temporary cache key must not make a previously supported draw
        // cross its working-buffer limit. Drop it and retain the exact cold
        // renderer's allocation/fallback contract.
        drop(key);
        return coverage_uncached(path, matrix, clip, grid, 0)
            .map(|(shader, bounds, _)| (shader, bounds));
    };
    mask_cache::insert(
        key,
        shader.clone(),
        bounds,
        bytes,
        super::reuse::cache_headroom(),
    );
    Some((shader, bounds))
}

/// A zero grid samples the current point camera; one/two resolve four jittered
/// cameras at that physical grid. Temporary counts/triangles leave before the
/// returned immutable mask; the caller may retain that mask in its cache.
fn coverage_uncached(
    path: &Path,
    matrix: &Matrix,
    clip: IRect,
    grid: u32,
    key_bytes: usize,
) -> Option<(Shader, Rect, usize)> {
    if !matches!(grid, 0..=2) || !matrix.is_finite() || matrix.has_perspective() {
        return None;
    }
    let inverse = matrix.invert()?;
    if !inverse.is_finite() {
        return None;
    }
    let (mut bounds, _) = matrix.map_rect(path.bounds());
    bounds.outset((1.0, 1.0));
    if !bounds.is_finite() || !bounds.intersect(Rect::from_irect(clip)) {
        return None;
    }
    bounds = Rect::new(
        bounds.left.floor(),
        bounds.top.floor(),
        bounds.right.ceil(),
        bounds.bottom.ceil(),
    );
    if bounds.is_empty()
        || bounds.width() > 8192.0
        || bounds.height() > 8192.0
        || [bounds.left, bounds.top, bounds.right, bounds.bottom]
            .iter()
            .any(|v| v.abs() * grid.max(1) as f32 > MAX_DEVICE_COORDINATE)
    {
        return None;
    }
    let (width, height) = (bounds.width() as usize, bounds.height() as usize);
    let pixels = width.checked_mul(height)?;
    let point = grid == 0;
    let grid = grid.max(1) as usize;
    let source_pixels = pixels.checked_mul(grid * grid)?;
    let triangles = triangles(path)?;
    let working = source_pixels
        .checked_mul(4)?
        .checked_add(pixels.checked_mul(2)?)?
        .max(pixels.checked_mul(6)?)
        .checked_add(
            triangles
                .capacity()
                .checked_mul(std::mem::size_of::<Triangle>())?,
        )?;
    let working = working.checked_add(key_bytes)?;
    if working > MAX_WORKING_BYTES {
        return None;
    }
    let mut counts = Vec::new();
    counts.try_reserve_exact(pixels).ok()?;
    counts.resize(pixels, 0u16);
    let mut stencil = Vec::new();
    stencil.try_reserve_exact(source_pixels).ok()?;
    stencil.resize(source_pixels, 0i32);
    let passes = if point { 1 } else { 4 };
    for jitter in JITTERS.iter().take(passes) {
        stencil.fill(0);
        let camera = Camera {
            matrix,
            grid: grid as i32,
            jitter: *jitter,
            point,
        };
        let mut target = Target {
            bounds,
            stencil: &mut stencil,
            width: width * grid,
            height: height * grid,
        };
        for triangle in &triangles {
            raster(*triangle, &camera, &mut target)?;
        }
        for y in 0..height {
            for x in 0..width {
                for dy in 0..grid {
                    for dx in 0..grid {
                        let winding = stencil[(y * grid + dy) * width * grid + x * grid + dx];
                        let filled = if path.fill_type() == PathFillType::EvenOdd {
                            winding & 1 != 0
                        } else {
                            winding != 0
                        };
                        counts[y * width + x] += u16::from(filled);
                    }
                }
            }
        }
    }
    drop(stencil);
    drop(triangles);
    // Exact IEEE binary16 encodings of n/16. Grid1 and point use every fourth
    // or sixteenth entry. This keeps fractional coverage, not an A8 rounding.
    const HALF: [u16; 17] = [
        0, 0x2c00, 0x3000, 0x3200, 0x3400, 0x3500, 0x3600, 0x3700, 0x3800, 0x3880, 0x3900, 0x3980,
        0x3a00, 0x3a80, 0x3b00, 0x3b80, 0x3c00,
    ];
    let denominator = passes * grid * grid;
    let bytes: Vec<_> = counts
        .into_iter()
        .flat_map(|n| HALF[usize::from(n) * 16 / denominator].to_ne_bytes())
        .collect();
    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::A16Float,
        AlphaType::Premul,
        None,
    );
    let image = skia_safe::images::raster_from_data(&info, Data::new_copy(&bytes), width * 2)?;
    let image_to_local = Matrix::concat(&inverse, &Matrix::translate((bounds.left, bounds.top)));
    let shader = image.to_shader(
        (TileMode::Decal, TileMode::Decal),
        SamplingOptions::default(),
        &image_to_local,
    )?;
    let (local_bounds, _) = inverse.map_rect(bounds);
    Some((shader, local_bounds, bytes.len()))
}

#[cfg(test)]
#[path = "vector_text_stencil_tests.rs"]
mod tests;
