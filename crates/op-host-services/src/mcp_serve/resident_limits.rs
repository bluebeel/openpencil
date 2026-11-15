//! Alternate-document admission policy. These are accounting limits, not RSS.
//!
//! Keep complete design files usable while bounding retained alternate state.
//! The primary document, renderer and request working storage are additional.

const DEFAULT_ACCOUNTED_BYTES: usize = 2 * 1024 * 1024 * 1024;
const DEFAULT_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ACCOUNTED_BYTES: u64 = 4_000_000_000;
const MAX_SOURCE_BYTES: u64 = 1024 * 1024 * 1024;
const ACCOUNTED_ENV: &str = "OPENPENCIL_MCP_RESIDENT_BYTES";
const SOURCE_ENV: &str = "OPENPENCIL_MCP_SOURCE_BYTES";

pub(super) struct ResidentLimits {
    pub accounted_bytes: usize,
    pub source_bytes: u64,
}

impl ResidentLimits {
    pub fn from_environment() -> Self {
        Self::read(|name| std::env::var(name).ok())
    }

    fn read(value: impl Fn(&str) -> Option<String>) -> Self {
        fn bytes(value: Option<String>, fallback: u64, maximum: u64) -> u64 {
            value
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|value| *value > 0)
                .map_or(fallback, |value| value.min(maximum))
        }
        Self {
            accounted_bytes: bytes(
                value(ACCOUNTED_ENV),
                DEFAULT_ACCOUNTED_BYTES as u64,
                MAX_ACCOUNTED_BYTES,
            ) as usize,
            source_bytes: bytes(value(SOURCE_ENV), DEFAULT_SOURCE_BYTES, MAX_SOURCE_BYTES),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_design_sources_fit_default_admission_policy() {
        let limits = ResidentLimits::read(|_| None);
        assert!(limits.source_bytes > 205_742_550);
        assert_eq!(limits.accounted_bytes, 2_147_483_648);
        assert_eq!(limits.source_bytes, 536_870_912);
    }

    #[test]
    fn explicit_smaller_limits_and_bounded_large_limits_are_supported() {
        let small = ResidentLimits::read(|_| Some("1048576".into()));
        assert_eq!(small.accounted_bytes, 1_048_576);
        assert_eq!(small.source_bytes, 1_048_576);
        let large = ResidentLimits::read(|_| Some(u64::MAX.to_string()));
        assert_eq!(large.accounted_bytes as u64, MAX_ACCOUNTED_BYTES);
        assert_eq!(large.source_bytes, MAX_SOURCE_BYTES);
    }

    #[test]
    fn invalid_limits_keep_finite_defaults() {
        for value in ["0", "-1", "nan", "18446744073709551616"] {
            let limits = ResidentLimits::read(|_| Some(value.into()));
            assert_eq!(limits.accounted_bytes, DEFAULT_ACCOUNTED_BYTES);
            assert_eq!(limits.source_bytes, DEFAULT_SOURCE_BYTES);
        }
    }
}
