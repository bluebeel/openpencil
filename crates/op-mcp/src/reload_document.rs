//! Explicit recovery of the live host's original bound document.
//!
//! File routing and document replacement belong to the host authority. The
//! ordinary registry has no such authority, so it returns an explicit error
//! rather than pretending that a metadata read or open_document is a reload.

use std::collections::BTreeMap;

use crate::{McpTool, ToolErrorCode, ToolOutcome};

pub struct ReloadDocument;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadArgumentError {
    UnsupportedArgument(String),
}

impl std::fmt::Display for ReloadArgumentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedArgument(name) => write!(
                formatter,
                "reload_document accepts only optional agentId; '{name}' is not supported. It always recovers the original bound primary file and never replays a failed request"
            ),
        }
    }
}

impl std::error::Error for ReloadArgumentError {}

pub fn validate_arguments(arguments: &BTreeMap<String, String>) -> Result<(), ReloadArgumentError> {
    match arguments.keys().find(|name| name.as_str() != "agentId") {
        Some(name) => Err(ReloadArgumentError::UnsupportedArgument(name.clone())),
        None => Ok(()),
    }
}

impl McpTool for ReloadDocument {
    fn name(&self) -> &str {
        "reload_document"
    }

    fn call(&self, arguments: &BTreeMap<String, String>) -> ToolOutcome {
        if let Err(error) = validate_arguments(arguments) {
            return ToolOutcome::Err(ToolErrorCode::InvalidArgument, error.to_string());
        }
        ToolOutcome::Err(
            ToolErrorCode::ToolFailed,
            "reload-document-unavailable: this transport has no live bound-file recovery authority; connect to the bound daemon's full /mcp endpoint. No request was replayed".into(),
        )
    }
}
