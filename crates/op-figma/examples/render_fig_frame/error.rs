use super::selector_error::SelectorError;
use op_figma::FigParseError;
use op_host_services::export::ExportError;
use serde_json::{json, Value};
use std::{
    fmt, io,
    num::{ParseFloatError, ParseIntError},
};

#[derive(Debug, Clone, Copy)]
pub enum IoOperation {
    ReadFont,
    ReadInput,
    WriteDocument,
    WriteScene,
    ReadPng,
}
#[derive(Debug)]
pub enum RenderFigError {
    Usage {
        provided: usize,
    },
    MissingOptionValue {
        option: String,
    },
    UnknownOption {
        option: String,
    },
    ConflictingSelectors,
    Selector(SelectorError),
    BoundsParse(serde_json::Error),
    InvalidExpectedBounds {
        bounds: [f64; 4],
    },
    SizeSyntax {
        value: String,
    },
    DimensionParse {
        axis: &'static str,
        value: String,
        source: ParseIntError,
    },
    ZeroPixels {
        width: u32,
        height: u32,
    },
    ScaleParse {
        value: String,
        source: ParseFloatError,
    },
    ScaleOutOfRange {
        scale: f32,
    },
    Io {
        operation: IoOperation,
        path: String,
        source: io::Error,
    },
    FontRegistration {
        path: String,
        reason: String,
    },
    Prepare(FigParseError),
    Convert(FigParseError),
    PageMatchCount {
        selector: String,
        matches: usize,
    },
    ConvertedPageMissing,
    SceneNodeMissing {
        id: String,
    },
    HiddenNode {
        id: String,
    },
    NoPaintedBounds {
        id: String,
    },
    InvalidPaintedBounds {
        actual: [f64; 4],
    },
    BoundsMismatch {
        expected: [f64; 4],
        actual: [f64; 4],
        tolerance: f64,
    },
    PixelMismatch {
        expected: (u32, u32),
        actual: (u32, u32),
    },
    InvalidPng {
        path: String,
    },
    DumpSerialize(serde_json::Error),
    Export(ExportError),
}
impl From<SelectorError> for RenderFigError {
    fn from(source: SelectorError) -> Self {
        Self::Selector(source)
    }
}
impl From<ExportError> for RenderFigError {
    fn from(source: ExportError) -> Self {
        Self::Export(source)
    }
}
impl fmt::Display for RenderFigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage { .. } => write!(
                f,
                "Usage: render_fig_frame <input.fig> <page-id-or-name> <top-level-name> <output.png> [font.ttf ...] [--node-path JSON | --node-id ID] [--expect-bounds JSON] [--expect-size WxH] [--scale 0.5..8]"
            ),
            Self::MissingOptionValue { option } => write!(f, "Missing value for {option}"),
            Self::UnknownOption { option } => write!(f, "Unknown option {option}"),
            Self::ConflictingSelectors => write!(f, "Use only one explicit node selector"),
            Self::Selector(error) => error.fmt(f),
            Self::BoundsParse(source) => write!(f, "Invalid expected bounds: {source}"),
            Self::InvalidExpectedBounds { .. } => write!(
                f,
                "Expected bounds must be finite and have positive dimensions"
            ),
            Self::SizeSyntax { .. } => write!(f, "Expected size must be WxH"),
            Self::DimensionParse { axis, .. } => write!(f, "Invalid expected {axis}"),
            Self::ZeroPixels { .. } => write!(f, "Expected pixels must be positive"),
            Self::ScaleParse { .. } => write!(f, "Invalid scale"),
            Self::ScaleOutOfRange { .. } => write!(f, "Scale must be within 0.5..8"),
            Self::Io {
                operation,
                path,
                source,
            } => match operation {
                IoOperation::ReadFont => write!(f, "Read font {path}: {source}"),
                IoOperation::ReadInput => write!(f, "Read input: {source}"),
                IoOperation::WriteDocument => write!(f, "Write document dump: {source}"),
                IoOperation::WriteScene => write!(f, "Write scene dump: {source}"),
                IoOperation::ReadPng => write!(f, "Read exported PNG header: {source}"),
            },
            Self::FontRegistration { path, reason } => {
                write!(f, "Register font {path}: {reason:?}")
            }
            Self::Prepare(source) => write!(f, "Prepare .fig: {source}"),
            Self::Convert(source) => write!(f, "Convert page: {source}"),
            Self::PageMatchCount { matches, .. } => write!(
                f,
                "Page selector must match exactly once; matched {matches}"
            ),
            Self::ConvertedPageMissing => write!(f, "Converted page missing"),
            Self::SceneNodeMissing { .. } => write!(f, "Selected scene node missing"),
            Self::HiddenNode { .. } => write!(f, "Selected scene node is hidden"),
            Self::NoPaintedBounds { .. } => write!(f, "Selected node has no painted bounds"),
            Self::InvalidPaintedBounds { actual } => {
                write!(f, "Invalid painted bounds: {actual:?}")
            }
            Self::BoundsMismatch {
                expected, actual, ..
            } => write!(
                f,
                "Painted bounds mismatch: expected {:?}, got {actual:?}",
                Some(expected)
            ),
            Self::PixelMismatch { expected, actual } => write!(
                f,
                "PNG dimensions mismatch: expected {:?}, got {actual:?}",
                Some(expected)
            ),
            Self::InvalidPng { .. } => write!(f, "Export did not produce a PNG"),
            Self::DumpSerialize(source) => source.fmt(f),
            Self::Export(source) => write!(f, "Export PNG: {source}"),
        }
    }
}
impl std::error::Error for RenderFigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Selector(source) => Some(source),
            Self::BoundsParse(source) | Self::DumpSerialize(source) => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::DimensionParse { source, .. } => Some(source),
            Self::ScaleParse { source, .. } => Some(source),
            Self::Export(source) => Some(source),
            _ => None,
        }
    }
}
impl RenderFigError {
    pub fn io(operation: IoOperation, path: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
    pub fn detail(&self) -> Value {
        match self {
            Self::Usage { provided } => json!({"kind":"usage","provided":provided,"minimum":4}),
            Self::MissingOptionValue { option } => {
                json!({"kind":"missing-option-value","option":option})
            }
            Self::UnknownOption { option } => json!({"kind":"unknown-option","option":option}),
            Self::ConflictingSelectors => json!({"kind":"conflicting-selectors"}),
            Self::Selector(source) => source.detail(),
            Self::BoundsParse(source) => {
                json!({"kind":"bounds-json","line":source.line(),"column":source.column()})
            }
            Self::InvalidExpectedBounds { bounds } => {
                json!({"kind":"invalid-expected-bounds","bounds":bounds})
            }
            Self::SizeSyntax { value } => json!({"kind":"pixel-size-syntax","value":value}),
            Self::DimensionParse { axis, value, .. } => {
                json!({"kind":"dimension-parse","axis":axis,"value":value})
            }
            Self::ZeroPixels { width, height } => {
                json!({"kind":"zero-pixels","width":width,"height":height})
            }
            Self::ScaleParse { value, .. } => json!({"kind":"scale-parse","value":value}),
            Self::ScaleOutOfRange { scale } => {
                json!({"kind":"scale-out-of-range","scale":scale,"minimum":0.5,"maximum":8})
            }
            Self::Io {
                operation,
                path,
                source,
            } => {
                json!({"kind":"io","operation":format!("{operation:?}"),"path":path,"ioKind":format!("{:?}",source.kind())})
            }
            Self::FontRegistration { path, reason } => {
                json!({"kind":"font-registration","path":path,"reason":reason})
            }
            Self::Prepare(source) => json!({"kind":"prepare","source":source.to_string()}),
            Self::Convert(source) => json!({"kind":"convert","source":source.to_string()}),
            Self::PageMatchCount { selector, matches } => {
                json!({"kind":"page-match-count","selector":selector,"matches":matches})
            }
            Self::ConvertedPageMissing => json!({"kind":"converted-page-missing"}),
            Self::SceneNodeMissing { id } => json!({"kind":"scene-node-missing","id":id}),
            Self::HiddenNode { id } => json!({"kind":"hidden-node","id":id}),
            Self::NoPaintedBounds { id } => json!({"kind":"no-painted-bounds","id":id}),
            Self::InvalidPaintedBounds { actual } => {
                json!({"kind":"invalid-painted-bounds","actual":actual})
            }
            Self::BoundsMismatch {
                expected,
                actual,
                tolerance,
            } => {
                json!({"kind":"bounds-mismatch","expected":expected,"actual":actual,"tolerance":tolerance})
            }
            Self::PixelMismatch { expected, actual } => {
                json!({"kind":"pixel-mismatch","expected":{"width":expected.0,"height":expected.1},"actual":{"width":actual.0,"height":actual.1}})
            }
            Self::InvalidPng { path } => json!({"kind":"invalid-png","path":path}),
            Self::DumpSerialize(source) => json!({"kind":"dump-json","source":source.to_string()}),
            Self::Export(source) => json!({"kind":"export","source":source.to_string()}),
        }
    }
}
