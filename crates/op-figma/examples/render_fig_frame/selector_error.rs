use serde_json::{json, Value};
use std::fmt;

#[derive(Debug)]
pub enum SelectorError {
    PathJson(serde_json::Error),
    PathArrayRequired,
    SegmentType {
        at: usize,
    },
    UnknownField {
        at: usize,
        field: String,
    },
    MissingName {
        at: usize,
    },
    InvalidIndex {
        at: usize,
        value: Value,
    },
    IndexOutOfRange {
        at: usize,
        name: String,
        index: usize,
        matches: usize,
    },
    PathMatchCount {
        at: usize,
        name: String,
        matches: usize,
    },
    IdMatchCount {
        id: String,
        matches: usize,
    },
    EmptyPath,
}
impl fmt::Display for SelectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PathJson(error) => write!(f, "Invalid node path: {error}"),
            Self::PathArrayRequired => write!(f, "Node path must be a nonempty JSON array"),
            Self::SegmentType { .. } => {
                write!(f, "Path segments must be names or {{name,index}} objects")
            }
            Self::UnknownField { .. } => write!(f, "Unknown node-path segment field"),
            Self::MissingName { .. } => write!(f, "Node-path segment requires a name"),
            Self::InvalidIndex { .. } => write!(f, "Node-path index must be a nonnegative integer"),
            Self::IndexOutOfRange {
                name,
                index,
                matches,
                ..
            } => write!(
                f,
                "Node-path segment {name:?} index {index} out of range ({matches} matches)"
            ),
            Self::PathMatchCount { name, matches, .. } => write!(
                f,
                "Node-path segment {name:?} must match exactly once; matched {matches}"
            ),
            Self::IdMatchCount { matches, .. } => {
                write!(f, "Node ID must match exactly once; matched {matches}")
            }
            Self::EmptyPath => write!(f, "Empty node path"),
        }
    }
}
impl std::error::Error for SelectorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PathJson(error) => Some(error),
            _ => None,
        }
    }
}
impl From<serde_json::Error> for SelectorError {
    fn from(source: serde_json::Error) -> Self {
        Self::PathJson(source)
    }
}
impl SelectorError {
    pub fn detail(&self) -> Value {
        match self {
            Self::PathJson(source) => {
                json!({"kind":"selector-json","line":source.line(),"column":source.column()})
            }
            Self::PathArrayRequired => json!({"kind":"selector-path-array-required"}),
            Self::SegmentType { at } => json!({"kind":"selector-segment-type","at":at}),
            Self::UnknownField { at, field } => {
                json!({"kind":"selector-unknown-field","at":at,"field":field})
            }
            Self::MissingName { at } => json!({"kind":"selector-missing-name","at":at}),
            Self::InvalidIndex { at, value } => {
                json!({"kind":"selector-invalid-index","at":at,"value":value})
            }
            Self::IndexOutOfRange {
                at,
                name,
                index,
                matches,
            } => {
                json!({"kind":"selector-index-out-of-range","at":at,"name":name,"index":index,"matches":matches})
            }
            Self::PathMatchCount { at, name, matches } => {
                json!({"kind":"selector-match-count","at":at,"name":name,"matches":matches})
            }
            Self::IdMatchCount { id, matches } => {
                json!({"kind":"selector-id-match-count","id":id,"matches":matches})
            }
            Self::EmptyPath => json!({"kind":"selector-empty-path"}),
        }
    }
}
