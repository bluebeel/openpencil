//! Borrowed SVG grammar reader with absolute f64 coordinates.

use super::Segment;
use svgtypes::{PathParser, PathSegment};

#[derive(Debug)]
pub(crate) enum PathDataError {
    Syntax(svgtypes::Error),
    NonFinite { command: u8 },
}

impl std::fmt::Display for PathDataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax(error) => std::fmt::Display::fmt(error, f),
            Self::NonFinite { command } => {
                write!(
                    f,
                    "non-finite SVG path coordinates for {}",
                    char::from(*command)
                )
            }
        }
    }
}

impl std::error::Error for PathDataError {}

pub(crate) struct AbsoluteSegments<'a> {
    source: PathParser<'a>,
    current: (f64, f64),
    start: (f64, f64),
    failed: bool,
}

impl<'a> AbsoluteSegments<'a> {
    pub(crate) fn new(source: &'a str) -> Self {
        Self {
            source: PathParser::from(source),
            current: (0.0, 0.0),
            start: (0.0, 0.0),
            failed: false,
        }
    }
}

impl Iterator for AbsoluteSegments<'_> {
    type Item = Result<Segment, PathDataError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        let raw = match self.source.next()? {
            Ok(raw) => raw,
            Err(error) => {
                self.failed = true;
                return Some(Err(PathDataError::Syntax(error)));
            }
        };
        let point = |abs, x, y| {
            if abs {
                (x, y)
            } else {
                (self.current.0 + x, self.current.1 + y)
            }
        };
        let segment = match raw {
            PathSegment::MoveTo { abs, x, y } => {
                let (x, y) = point(abs, x, y);
                self.start = (x, y);
                Segment::Move(x, y)
            }
            PathSegment::LineTo { abs, x, y } => {
                let (x, y) = point(abs, x, y);
                Segment::Line(x, y)
            }
            PathSegment::HorizontalLineTo { abs, x } => {
                Segment::HLine(if abs { x } else { self.current.0 + x })
            }
            PathSegment::VerticalLineTo { abs, y } => {
                Segment::VLine(if abs { y } else { self.current.1 + y })
            }
            PathSegment::CurveTo {
                abs,
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let (x1, y1) = point(abs, x1, y1);
                let (x2, y2) = point(abs, x2, y2);
                let (x, y) = point(abs, x, y);
                Segment::Cubic(x1, y1, x2, y2, x, y)
            }
            PathSegment::SmoothCurveTo { abs, x2, y2, x, y } => {
                let (x2, y2) = point(abs, x2, y2);
                let (x, y) = point(abs, x, y);
                Segment::SmoothCubic(x2, y2, x, y)
            }
            PathSegment::Quadratic { abs, x1, y1, x, y } => {
                let (x1, y1) = point(abs, x1, y1);
                let (x, y) = point(abs, x, y);
                Segment::Quad(x1, y1, x, y)
            }
            PathSegment::SmoothQuadratic { abs, x, y } => {
                let (x, y) = point(abs, x, y);
                Segment::SmoothQuad(x, y)
            }
            PathSegment::EllipticalArc {
                abs,
                rx,
                ry,
                x_axis_rotation,
                large_arc,
                sweep,
                x,
                y,
            } => {
                let (x, y) = point(abs, x, y);
                Segment::Arc(
                    rx.abs(),
                    ry.abs(),
                    x_axis_rotation,
                    u8::from(large_arc).into(),
                    u8::from(sweep).into(),
                    x,
                    y,
                )
            }
            PathSegment::ClosePath { .. } => Segment::Close,
        };
        // The grammar parser rejects non-finite input; relative additions can
        // still overflow, so check the normalized segment before publishing it.
        if !segment.is_finite() {
            self.failed = true;
            return Some(Err(PathDataError::NonFinite {
                command: raw.command(),
            }));
        }
        self.current = match segment {
            Segment::Move(x, y)
            | Segment::Line(x, y)
            | Segment::SmoothQuad(x, y)
            | Segment::Quad(_, _, x, y)
            | Segment::SmoothCubic(_, _, x, y)
            | Segment::Cubic(_, _, _, _, x, y)
            | Segment::Arc(_, _, _, _, _, x, y) => (x, y),
            Segment::HLine(x) => (x, self.current.1),
            Segment::VLine(y) => (self.current.0, y),
            Segment::Close => self.start,
        };
        Some(Ok(segment))
    }
}

impl Segment {
    pub(super) fn is_finite(&self) -> bool {
        let finite = |values: &[f64]| values.iter().all(|value| value.is_finite());
        match *self {
            Self::Move(x, y) | Self::Line(x, y) | Self::SmoothQuad(x, y) => finite(&[x, y]),
            Self::HLine(x) | Self::VLine(x) => x.is_finite(),
            Self::Quad(x1, y1, x, y) | Self::SmoothCubic(x1, y1, x, y) => finite(&[x1, y1, x, y]),
            Self::Cubic(x1, y1, x2, y2, x, y) => finite(&[x1, y1, x2, y2, x, y]),
            Self::Arc(rx, ry, rotation, large, sweep, x, y) => {
                finite(&[rx, ry, rotation, large, sweep, x, y])
            }
            Self::Close => true,
        }
    }
}
