use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentContextError {
    Busy,
    InvalidAgent,
    Capacity {
        resource: &'static str,
        limit: usize,
    },
    Denied {
        message: String,
    },
    ClipboardSerialization,
    OversizedSelection {
        limit: usize,
    },
    UnaccountedClipboard,
}

impl fmt::Display for AgentContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("agent-context-busy: this agentId already has an active call"),
            Self::InvalidAgent => f.write_str("agentId must contain 1–96 ASCII letters, digits, '.', '_', ':' or '-'"),
            Self::Capacity { resource, limit } => write!(f, "agent-context-capacity: {resource} limit reached ({limit}); retry after releasing context resources"),
            Self::Denied { message } => f.write_str(message),
            Self::ClipboardSerialization => f.write_str("agent-context-capacity: clipboard could not be accounted"),
            Self::OversizedSelection { limit } => write!(f, "agent-context-capacity: selection exceeds {limit} entries/byte allowance; selection discarded, document edits remain shared"),
            Self::UnaccountedClipboard => f.write_str("agent-context-capacity: new clipboard discarded; document edits remain shared"),
        }
    }
}
impl std::error::Error for AgentContextError {}
