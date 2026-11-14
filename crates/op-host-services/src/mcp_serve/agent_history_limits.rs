//! Shared runtime history policy, independent of persisted document payloads.
use op_editor_core::{HistoryLimitError, HistoryLimits};
use std::sync::OnceLock;
#[derive(Debug, Clone)]
pub(super) enum ConfigError {
    Number { name: &'static str },
    Bounds(HistoryLimitError),
}
impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number { name } => write!(f, "{name} must be an unsigned integer"),
            Self::Bounds(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for ConfigError {}
fn number(name: &'static str, default: usize) -> Result<usize, ConfigError> {
    match std::env::var(name) {
        Ok(value) => value.parse().map_err(|_| ConfigError::Number { name }),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(_) => Err(ConfigError::Number { name }),
    }
}
pub(super) fn configured() -> Result<HistoryLimits, ConfigError> {
    static CONFIG: OnceLock<Result<HistoryLimits, ConfigError>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let default = HistoryLimits::default();
            let entries = number("OPENPENCIL_HISTORY_ENTRIES", default.entries)?;
            let legacy = number(
                "OPENPENCIL_HISTORY_LEGACY_ENTRIES",
                default.legacy_entries.min(entries),
            )?;
            let bytes = number("OPENPENCIL_HISTORY_BYTES", default.bytes)?;
            HistoryLimits::checked(entries, legacy, bytes).map_err(ConfigError::Bounds)
        })
        .clone()
}
