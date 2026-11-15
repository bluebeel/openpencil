//! File entry point: release the mapped header before growing the typed tree.
use super::{header::Header, CanonicalLoad, CompatibilityReport};
use crate::EditorMetaExtraction;
use std::{
    fs::File,
    io::{BufReader, Seek, SeekFrom},
};

pub struct CanonicalFileLoad {
    pub canonical: CanonicalLoad,
    pub editor_meta: Option<EditorMetaExtraction>,
}
#[derive(Debug)]
pub enum CanonicalFileLoadError {
    Io(std::io::Error),
    InvalidUtf8(std::str::Utf8Error),
    Schema {
        error: jian_ops_schema::OpsSchemaError,
        legacy_doc_payload: bool,
    },
}
impl std::fmt::Display for CanonicalFileLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::InvalidUtf8(e) => write!(f, "{e}"),
            Self::Schema { error, .. } => write!(f, "{error}"),
        }
    }
}
impl std::error::Error for CanonicalFileLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::InvalidUtf8(e) => Some(e),
            Self::Schema { error, .. } => Some(error),
        }
    }
}
fn schema(
    error: jian_ops_schema::OpsSchemaError,
    legacy_doc_payload: bool,
) -> CanonicalFileLoadError {
    CanonicalFileLoadError::Schema {
        error,
        legacy_doc_payload,
    }
}
fn mapped(file: &File) -> Result<memmap2::Mmap, CanonicalFileLoadError> {
    // SAFETY: the caller provides a read-only document handle. Product saves
    // atomically replace inodes; they never truncate a mapped input in place.
    unsafe { memmap2::MmapOptions::new().map(file) }.map_err(CanonicalFileLoadError::Io)
}
/// Load one already-authorized file. Header and body use the same open inode,
/// including when another process atomically replaces its pathname. The input
/// must not be truncated in place while it is mapped. A cloned file cursor is
/// rewound for typed decoding; hosts call this with their fresh exclusive file.
pub fn load_canonical_file_with_compatibility(
    file: &File,
) -> Result<CanonicalFileLoad, CanonicalFileLoadError> {
    if file.metadata().map_err(CanonicalFileLoadError::Io)?.len() == 0 {
        let canonical = super::super::load_canonical_dom_with_compatibility("")
            .map_err(|e| schema(e, false))?;
        return Ok(CanonicalFileLoad {
            canonical,
            editor_meta: None,
        });
    }
    let (raw_header, editor_meta, legacy_doc_payload) = {
        let bytes = mapped(file)?;
        let source = std::str::from_utf8(&bytes).map_err(CanonicalFileLoadError::InvalidUtf8)?;
        let editor_meta = crate::extract_editor_meta_checked(source)
            .map_err(|error| schema(jian_ops_schema::OpsSchemaError::Json(error), false))?;
        let probe = serde_json::from_str::<Header>(source).ok();
        let legacy_doc_payload = probe.as_ref().is_some_and(|h| h.legacy_doc_payload);
        match probe {
            Some(header) if !header.legacy => (header.raw, editor_meta, legacy_doc_payload),
            _ => {
                let canonical = super::super::load_canonical_dom_with_compatibility(source)
                    .map_err(|e| schema(e, legacy_doc_payload))?;
                return Ok(CanonicalFileLoad {
                    canonical,
                    editor_meta,
                });
            }
        }
    };
    // The source mapping is gone before the complete typed document grows.
    let mut reader = file.try_clone().map_err(CanonicalFileLoadError::Io)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(CanonicalFileLoadError::Io)?;
    match jian_ops_schema::compat::load_reader_with_header(
        BufReader::with_capacity(1024 * 1024, reader),
        raw_header,
    ) {
        Ok(loaded) => Ok(CanonicalFileLoad {
            canonical: CanonicalLoad {
                loaded,
                compatibility: CompatibilityReport::default(),
            },
            editor_meta,
        }),
        Err(_) => {
            // Unsupported legacy shapes, duplicate aliases, and deep input
            // retain the established stack-protected compatibility pipeline.
            let bytes = mapped(file)?;
            let source =
                std::str::from_utf8(&bytes).map_err(CanonicalFileLoadError::InvalidUtf8)?;
            let canonical = super::super::load_canonical_dom_with_compatibility(source)
                .map_err(|e| schema(e, legacy_doc_payload))?;
            Ok(CanonicalFileLoad {
                canonical,
                editor_meta,
            })
        }
    }
}
