//! Explicit local/managed headless font resources, loaded before any requests.
//!
//! Use the platform path-list syntax, like PATH, to select exact font files.
//! Unconfigured services preserve their existing embedded/system fallback.

use std::{
    collections::HashSet,
    ffi::OsStr,
    fmt,
    io::Read,
    path::{Path, PathBuf},
};

pub(crate) const FONT_FILES_ENV: &str = "OPENPENCIL_FONT_FILES";
const MAX_FILES: usize = 64;
const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub(crate) enum FontStartupError {
    Io { path: PathBuf, message: String },
    Invalid { path: PathBuf, reason: String },
    Capacity { max_files: usize, max_bytes: usize },
}

impl fmt::Display for FontStartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "font resource {}: {message}", path.display()),
            Self::Invalid { path, reason } => {
                write!(f, "invalid font resource {}: {reason}", path.display())
            }
            Self::Capacity {
                max_files,
                max_bytes,
            } => write!(
                f,
                "configured fonts exceed {max_files} files / {max_bytes} bytes"
            ),
        }
    }
}

impl std::error::Error for FontStartupError {}

pub(crate) fn register_configured_fonts() -> Result<usize, FontStartupError> {
    let configured = std::env::var_os(FONT_FILES_ENV);
    load(configured.as_deref(), |path, bytes| {
        jian_skia::register_imported_font(bytes)
            .map(|_| ())
            .map_err(|reason| FontStartupError::Invalid {
                path: path.to_path_buf(),
                reason,
            })
    })
}

fn load(
    configured: Option<&OsStr>,
    mut register: impl FnMut(&Path, Vec<u8>) -> Result<(), FontStartupError>,
) -> Result<usize, FontStartupError> {
    let Some(configured) = configured.filter(|value| !value.is_empty()) else {
        return Ok(0);
    };
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    let mut total = 0usize;
    for path in std::env::split_paths(configured) {
        let canonical = path.canonicalize().map_err(|error| FontStartupError::Io {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if !seen.insert(canonical.clone()) {
            continue;
        }
        if files.len() >= MAX_FILES {
            return Err(FontStartupError::Capacity {
                max_files: MAX_FILES,
                max_bytes: MAX_BYTES,
            });
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&canonical)
            .and_then(|file| {
                file.take((MAX_BYTES - total + 1) as u64)
                    .read_to_end(&mut bytes)
            })
            .map_err(|error| FontStartupError::Io {
                path: canonical.clone(),
                message: error.to_string(),
            })?;
        total += bytes.len();
        if total > MAX_BYTES {
            return Err(FontStartupError::Capacity {
                max_files: MAX_FILES,
                max_bytes: MAX_BYTES,
            });
        }
        files.push((canonical, bytes));
    }
    let count = files.len();
    for (path, bytes) in files {
        register(&path, bytes)?;
    }
    Ok(count)
}

#[cfg(test)]
#[path = "headless_fonts_tests.rs"]
mod tests;
