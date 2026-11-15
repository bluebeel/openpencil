//! Stream encoded output to a sibling file and install only a complete result.

use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{ExportError, RasterFormat};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

struct PendingFile(PathBuf);
impl Drop for PendingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn io_error(error: io::Error) -> ExportError {
    ExportError::Write(error.to_string())
}

pub(crate) fn write_atomic(
    target: &Path,
    encode: impl FnOnce(&mut dyn Write) -> Result<(), ExportError>,
) -> Result<(), ExportError> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let name = target
        .file_name()
        .ok_or_else(|| ExportError::Write("export target has no file name".into()))?;
    let permissions = match fs::metadata(target) {
        Ok(meta) => Some(meta.permissions()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(io_error(error)),
    };
    let (pending, file) = (0..32)
        .find_map(|_| {
            let mut name = name.to_os_string();
            name.push(format!(
                ".op-export-{}-{}",
                std::process::id(),
                NEXT_FILE.fetch_add(1, Ordering::Relaxed)
            ));
            let path = parent.join(name);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => Some(Ok((PendingFile(path), file))),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(io_error(error))),
            }
        })
        .unwrap_or_else(|| {
            Err(ExportError::Write(
                "could not reserve a sibling export file".into(),
            ))
        })?;
    if let Some(permissions) = permissions {
        file.set_permissions(permissions).map_err(io_error)?;
    }
    let mut writer = BufWriter::with_capacity(64 * 1024, file);
    encode(&mut writer)?;
    writer.flush().map_err(io_error)?;
    drop(writer);
    fs::rename(&pending.0, target).map_err(io_error)?;
    Ok(())
}

// Skia returns bool for encoding. Retain the actual IO failure so disk errors
// are not incorrectly reported as an unsupported or failed codec.
pub(crate) struct CheckedWriter<'a> {
    writer: &'a mut dyn Write,
    error: Option<String>,
    written: usize,
}
impl<'a> CheckedWriter<'a> {
    pub(crate) fn new(writer: &'a mut dyn Write) -> Self {
        Self {
            writer,
            error: None,
            written: 0,
        }
    }
    pub(crate) fn finish_pdf(self) -> Result<(), ExportError> {
        if let Some(error) = self.error {
            Err(ExportError::Write(error))
        } else if self.written == 0 {
            Err(ExportError::PdfEncoderEmpty)
        } else {
            Ok(())
        }
    }
    pub(super) fn result(self, success: bool, format: RasterFormat) -> Result<(), ExportError> {
        if let Some(error) = self.error {
            Err(ExportError::Write(error))
        } else if success {
            Ok(())
        } else {
            Err(ExportError::Encode {
                format: format.user_label(),
            })
        }
    }
}
impl Write for CheckedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let result = self.writer.write(bytes);
        match &result {
            Ok(0) if !bytes.is_empty() => {
                self.error
                    .get_or_insert_with(|| io::Error::from(io::ErrorKind::WriteZero).to_string());
            }
            Ok(written) => {
                self.written = self.written.saturating_add(*written);
            }
            Err(error) => {
                self.error.get_or_insert_with(|| error.to_string());
            }
        }
        result
    }
    fn flush(&mut self) -> io::Result<()> {
        let result = self.writer.flush();
        if let Err(error) = &result {
            self.error.get_or_insert_with(|| error.to_string());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_encoding_preserves_destination_and_removes_sibling() {
        let dir =
            std::env::temp_dir().join(format!("op-streamed-export-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("original.png");
        fs::write(&target, b"original").unwrap();
        let result = write_atomic(&target, |writer| {
            writer.write_all(b"partial").unwrap();
            Err(ExportError::Encode { format: "PNG" })
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
