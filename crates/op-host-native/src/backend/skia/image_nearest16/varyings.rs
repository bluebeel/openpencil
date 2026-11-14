//! Float32 corner varyings and triangle setup for the imported point sampler.
//! The reference source pass uses last-provoking strips, a downward Vulkan
//! viewport, four subpixel bits, and staged row/column interpolation. Keep
//! these stages separate: an affine evaluated in the fragment is not identical.
use skia_safe::{Matrix, Rect};

pub(super) const FLOATS: usize = 154;
const JITTERS: [(f32, f32); 4] = [
    (0.625, 0.875),
    (0.375, 0.125),
    (0.125, 0.625),
    (0.875, 0.375),
];

#[derive(Clone, Copy, Default)]
struct Triangle {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    origin: [f32; 2],
}
#[derive(Clone, Copy, Default)]
struct Camera {
    triangles: [Triangle; 2],
    edge: [f32; 4],
    side: [f32; 4],
}

fn corner_uv(uv: [f32; 6], x: f32, y: f32) -> [f32; 2] {
    [uv[0] * x + uv[1] * y + uv[2], uv[3] * x + uv[4] * y + uv[5]]
}
fn triangle(
    points: [[f32; 2]; 4],
    values: [[f32; 2]; 4],
    mut indices: [usize; 3],
    screen: [[f32; 2]; 4],
) -> Option<Triangle> {
    // Equal minima perform both rotations. In particular the second strip
    // triangle can retain a different anchor than a geometric top-left sort.
    let y = indices.map(|i| screen[i][1]);
    let minimum = y.into_iter().fold(f32::INFINITY, f32::min);
    if minimum == y[1] {
        indices.rotate_left(1);
    }
    if minimum == y[2] {
        indices.rotate_left(2);
    }
    let [i0, i1, i2] = indices;
    let [x0, y0] = points[i0];
    let [x1, y1] = [points[i1][0] - x0, points[i1][1] - y0];
    let [x2, y2] = [points[i2][0] - x0, points[i2][1] - y0];
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
        for channel in 0..2 {
            coefficients[axis][channel] = (values[i0][channel] * m[0][axis]
                + values[i1][channel] * m[1][axis])
                + values[i2][channel] * m[2][axis];
        }
        coefficients[axis][2] = (m[0][axis] + m[1][axis]) + m[2][axis];
    }
    coefficients
        .iter()
        .flatten()
        .all(|x| x.is_finite())
        .then_some(Triangle {
            a: coefficients[0],
            b: coefficients[1],
            c: coefficients[2],
            origin: [x0, y0],
        })
}
fn camera(
    screen: [[f32; 2]; 4],
    uv: [[f32; 2]; 4],
    grid: f32,
    jitter: (f32, f32),
    point: bool,
) -> Option<Camera> {
    let projected = screen.map(|p| {
        if point {
            [p[0] - 0.5, 0.5 - p[1]]
        } else {
            [grid * p[0] - jitter.0, -grid * p[1] + jitter.1]
        }
    });
    let integers = projected.map(|p| p.map(|v| (v * 16.0).round_ties_even()));
    if !integers
        .iter()
        .flatten()
        .all(|v| v.is_finite() && v.abs() <= 8_388_607.0)
    {
        return None;
    }
    let points = integers.map(|p| p.map(|v| v / 16.0));
    let triangles = [
        triangle(points, uv, [2, 0, 1], screen)?,
        triangle(points, uv, [3, 2, 1], screen)?,
    ];
    let base = integers[1];
    let dx = integers[2][0] - base[0];
    let dy = integers[2][1] - base[1];
    // A two-source-pixel border covers the draw/sample rounding envelope.
    // Every signed cross-product operand then fits 16 bits, so two products
    // and their difference fit signed 32. Larger quads keep the old sampler.
    for axis in 0..2 {
        let maximum = integers
            .iter()
            .map(|p| (p[axis] - base[axis]).abs())
            .fold(0.0f32, f32::max);
        if maximum + 32.0 > 32767.0 {
            return None;
        }
    }
    let side = f64::from(dx) * f64::from(integers[0][1] - base[1])
        - f64::from(dy) * f64::from(integers[0][0] - base[0]);
    if side == 0.0 {
        return None;
    }
    let inclusive = if dy != 0.0 {
        f64::from(dy) * side < 0.0
    } else {
        integers[0][1] > base[1]
    };
    Some(Camera {
        triangles,
        edge: [dx, dy, base[0], base[1]],
        side: [
            if side > 0.0 { 1.0 } else { -1.0 },
            if inclusive { 1.0 } else { 0.0 },
            0.0,
            0.0,
        ],
    })
}

/// No retained buffers. Unsupported/nonfinite/perspective or numerically unsafe
/// cameras return None and retain the established affine sampler.
pub(super) fn uniforms(
    device: &Matrix,
    rect: Rect,
    uv: [f32; 6],
    grid: f32,
) -> Option<[f32; FLOATS]> {
    if !device.is_finite()
        || device.has_perspective()
        || !rect.is_finite()
        || !uv.into_iter().all(f32::is_finite)
        || !matches!(grid, 0.0 | 1.0 | 2.0)
    {
        return None;
    }
    let corners = [
        [rect.left, rect.top],
        [rect.right, rect.top],
        [rect.left, rect.bottom],
        [rect.right, rect.bottom],
    ];
    let screen = corners.map(|p| {
        [
            device.scale_x() * p[0] + device.skew_x() * p[1] + device.translate_x(),
            device.skew_y() * p[0] + device.scale_y() * p[1] + device.translate_y(),
        ]
    });
    let inverse = device.invert()?;
    // Recover a device pixel center from shader-local coordinates only while
    // the Float32 round-trip error is well below half a pixel.
    let magnitude = corners
        .iter()
        .flatten()
        .chain([device.translate_x(), device.translate_y()].iter())
        .map(|v| v.abs())
        .fold(1.0f32, f32::max);
    let norm = device
        .scale_x()
        .abs()
        .max(device.skew_x().abs())
        .max(device.skew_y().abs())
        .max(device.scale_y().abs())
        .max(1.0);
    let inverse_norm = inverse
        .scale_x()
        .abs()
        .max(inverse.skew_x().abs())
        .max(inverse.skew_y().abs())
        .max(inverse.scale_y().abs())
        .max(1.0);
    if magnitude * norm * inverse_norm * (32.0 * f32::EPSILON) > 0.125 {
        return None;
    }
    let values = [
        corner_uv(uv, 0.0, 0.0),
        corner_uv(uv, 1.0, 0.0),
        corner_uv(uv, 0.0, 1.0),
        corner_uv(uv, 1.0, 1.0),
    ];
    let source_grid = grid.max(1.0);
    let mut cameras = [Camera::default(); 4];
    for (index, jitter) in JITTERS.into_iter().enumerate() {
        cameras[index] = camera(screen, values, source_grid, jitter, grid == 0.0)?;
    }
    let mut result = [0.0; FLOATS];
    let mut cursor = 0;
    let mut push = |values: &[f32]| {
        result[cursor..cursor + values.len()].copy_from_slice(values);
        cursor += values.len();
    };
    push(&[1.0, source_grid]);
    push(&[
        device.scale_x(),
        device.skew_x(),
        device.skew_y(),
        device.scale_y(),
    ]);
    push(&[device.translate_x(), device.translate_y(), 0.0, 0.0]);
    for field in 0..3 {
        for c in cameras {
            for t in c.triangles {
                push(match field {
                    0 => &t.a,
                    1 => &t.b,
                    _ => &t.c,
                });
            }
        }
    }
    for c in cameras {
        push(&[
            c.triangles[0].origin[0],
            c.triangles[0].origin[1],
            c.triangles[1].origin[0],
            c.triangles[1].origin[1],
        ]);
    }
    for c in cameras {
        push(&c.edge);
    }
    for c in cameras {
        push(&c.side);
    }
    debug_assert_eq!(cursor, FLOATS);
    Some(result)
}

#[cfg(test)]
#[path = "varyings_tests.rs"]
mod tests;
