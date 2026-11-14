//! Bound outline subdivision error in device pixels before scan conversion.
//!
//! Skia's non-AA curve scan converter uses a coarser subdivision than its GPU
//! path renderer. Flattening first keeps the same sample lattice and winding,
//! while removing that curve-dependent coverage bias.

use skia_safe::{Matrix, Path, PathBuilder, PathVerb, Point};

const MAX_DEVICE_ERROR: f32 = 1.0 / 64.0;
pub(super) const MAX_POINTS: usize = 256 * 1024;
const MAX_DEPTH: u32 = 16;

pub(super) fn flatten(path: &Path, matrix: &Matrix) -> Option<Path> {
    flatten_with_limit(path, matrix, MAX_POINTS)
}

fn flatten_with_limit(path: &Path, matrix: &Matrix, limit: usize) -> Option<Path> {
    if !matrix.is_finite() || matrix.has_perspective() || path.count_points() > limit {
        return None;
    }
    let mut output = Outline {
        builder: PathBuilder::new_with_fill_type(path.fill_type()),
        x_axis: matrix.map_vector((1.0, 0.0)),
        y_axis: matrix.map_vector((0.0, 1.0)),
        remaining: limit,
    };
    for record in path.iter() {
        let p = record.points();
        if p.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
            return None;
        }
        match record.verb() {
            PathVerb::Move => {
                output.reserve_point()?;
                output.builder.move_to(p[0]);
            }
            PathVerb::Line => output.line(p[1])?,
            PathVerb::Quad => output.quad([p[0], p[1], p[2]], 0)?,
            PathVerb::Cubic => output.cubic([p[0], p[1], p[2], p[3]], 0)?,
            PathVerb::Close => {
                output.builder.close();
            }
            PathVerb::Conic => output.conic([p[0], p[1], p[2]], record.conic_weight())?,
        }
    }
    Some(output.builder.detach())
}

struct Outline {
    builder: PathBuilder,
    x_axis: Point,
    y_axis: Point,
    remaining: usize,
}

impl Outline {
    fn reserve_point(&mut self) -> Option<()> {
        self.remaining = self.remaining.checked_sub(1)?;
        Some(())
    }

    fn line(&mut self, p: Point) -> Option<()> {
        self.reserve_point()?;
        self.builder.line_to(p);
        Some(())
    }

    fn device_length(&self, p: Point) -> f32 {
        (p.x * self.x_axis.x + p.y * self.y_axis.x).hypot(p.x * self.x_axis.y + p.y * self.y_axis.y)
    }

    fn quad(&mut self, p: [Point; 3], depth: u32) -> Option<()> {
        // Maximum distance from the parameterized chord is |P''| / 8.
        let error = self.device_length(second_difference(p[0], p[1], p[2])) * 0.25;
        if error.is_finite() && error <= MAX_DEVICE_ERROR {
            return self.line(p[2]);
        }
        if !error.is_finite() || depth >= MAX_DEPTH {
            return None;
        }
        let a = midpoint(p[0], p[1]);
        let b = midpoint(p[1], p[2]);
        let m = midpoint(a, b);
        self.quad([p[0], a, m], depth + 1)?;
        self.quad([m, b, p[2]], depth + 1)
    }

    fn conic(&mut self, p: [Point; 3], weight: f32) -> Option<()> {
        if !weight.is_finite() || weight <= 0.0 {
            return None;
        }
        let h = |point: Point, weight: f64| {
            [
                f64::from(point.x) * weight,
                f64::from(point.y) * weight,
                weight,
            ]
        };
        self.rational([h(p[0], 1.0), h(p[1], f64::from(weight)), h(p[2], 1.0)], 0)
    }

    fn rational(&mut self, p: [[f64; 3]; 3], depth: u32) -> Option<()> {
        let project = |p: [f64; 3]| (p[0] / p[2], p[1] / p[2]);
        let [(x0, y0), (x1, y1), (x2, y2)] = p.map(project);
        let device = |x: f64, y: f64| {
            (
                x * f64::from(self.x_axis.x) + y * f64::from(self.y_axis.x),
                x * f64::from(self.x_axis.y) + y * f64::from(self.y_axis.y),
            )
        };
        let (dx, dy) = device(x2 - x0, y2 - y0);
        let (cx, cy) = device(x1 - x0, y1 - y0);
        let length = dx * dx + dy * dy;
        let t = if length > 0.0 {
            ((cx * dx + cy * dy) / length).clamp(0.0, 1.0)
        } else {
            0.0
        };
        // Positive rational weights keep the complete curve inside the
        // projected control triangle. Its distance to the chord segment is a
        // conservative device-space bound, including collinear overshoot.
        let error = (cx - t * dx).hypot(cy - t * dy);
        if !error.is_finite() {
            return None;
        }
        if error <= f64::from(MAX_DEVICE_ERROR) {
            let end = Point::new(x2 as f32, y2 as f32);
            if !end.x.is_finite() || !end.y.is_finite() {
                return None;
            }
            return self.line(end);
        }
        if depth >= MAX_DEPTH {
            return None;
        }
        let mid = |a: [f64; 3], b: [f64; 3]| {
            [
                (a[0] + b[0]) * 0.5,
                (a[1] + b[1]) * 0.5,
                (a[2] + b[2]) * 0.5,
            ]
        };
        let a = mid(p[0], p[1]);
        let b = mid(p[1], p[2]);
        let center = mid(a, b);
        self.rational([p[0], a, center], depth + 1)?;
        self.rational([center, b, p[2]], depth + 1)
    }

    fn cubic(&mut self, p: [Point; 4], depth: u32) -> Option<()> {
        // The second derivative is the interpolation of these two vectors;
        // its maximum length bounds distance from the parameterized chord.
        let a = self.device_length(second_difference(p[0], p[1], p[2]));
        let b = self.device_length(second_difference(p[1], p[2], p[3]));
        if !a.is_finite() || !b.is_finite() {
            return None;
        }
        let error = a.max(b) * 0.75;
        if error.is_finite() && error <= MAX_DEVICE_ERROR {
            return self.line(p[3]);
        }
        if !error.is_finite() || depth >= MAX_DEPTH {
            return None;
        }
        let a = midpoint(p[0], p[1]);
        let b = midpoint(p[1], p[2]);
        let c = midpoint(p[2], p[3]);
        let ab = midpoint(a, b);
        let bc = midpoint(b, c);
        let m = midpoint(ab, bc);
        self.cubic([p[0], a, ab, m], depth + 1)?;
        self.cubic([m, bc, c, p[3]], depth + 1)
    }
}

fn midpoint(a: Point, b: Point) -> Point {
    Point::new(a.x * 0.5 + b.x * 0.5, a.y * 0.5 + b.y * 0.5)
}

fn second_difference(a: Point, b: Point, c: Point) -> Point {
    Point::new(a.x - 2.0 * b.x + c.x, a.y - 2.0 * b.y + c.y)
}

#[cfg(test)]
#[path = "vector_text_curve_tests.rs"]
mod tests;
