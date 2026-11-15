//! Canonical typed loading without a whole-document compatibility DOM.
#[cfg(not(target_arch = "wasm32"))]
mod file;
mod header;
mod probe;
use super::{CanonicalLoad, CompatibilityReport};
#[cfg(not(target_arch = "wasm32"))]
pub use file::{load_canonical_file_with_compatibility, CanonicalFileLoad, CanonicalFileLoadError};
fn header(source: &str) -> Option<serde_json::Value> {
    let header: header::Header = serde_json::from_str(source).ok()?;
    (!header.legacy).then_some(header.raw)
}
pub(super) fn try_load_source(source: &str) -> Option<CanonicalLoad> {
    let header = header(source)?;
    let loaded = jian_ops_schema::compat::load_str_with_header(source, header).ok()?;
    Some(CanonicalLoad {
        loaded,
        compatibility: CompatibilityReport::default(),
    })
}

#[cfg(test)]
mod tests;
