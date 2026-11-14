//! Sibling-file replacement for the streaming Figma importer save.

use crate::cli_error::CliError;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

fn write_error(path: &str, error: std::io::Error) -> CliError {
    CliError::Io(format!("write {path:?}: {error}"))
}

fn create_sibling(destination: &Path) -> std::io::Result<(Temporary, File)> {
    let name = destination.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "output path must name a file",
        )
    })?;
    let directory = destination.parent().unwrap_or_else(|| Path::new("."));
    for _ in 0..128 {
        let mut temporary_name = std::ffi::OsString::from(".");
        temporary_name.push(name);
        temporary_name.push(format!(
            ".tmp-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let temporary_path = directory.join(temporary_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => {
                let temporary = Temporary(temporary_path);
                match std::fs::metadata(destination) {
                    Ok(metadata) if metadata.is_file() => {
                        file.set_permissions(metadata.permissions())?
                    }
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        return Err(error)
                    }
                    _ => {}
                }
                return Ok((temporary, file));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a sibling temporary output",
    ))
}

/// The requested path is untouched until serialization and buffered writes
/// succeed. A failed write, flush, or rename removes only our temporary file.
pub(super) fn write_atomically<T>(
    requested: &str,
    write: impl FnOnce(&mut BufWriter<File>) -> Result<T, CliError>,
) -> Result<T, CliError> {
    let destination = Path::new(requested);
    let (mut temporary, file) =
        create_sibling(destination).map_err(|e| write_error(requested, e))?;
    let mut output = BufWriter::with_capacity(64 * 1024, file);
    let result = write(&mut output)?;
    output.flush().map_err(|e| write_error(requested, e))?;
    drop(output);
    std::fs::rename(&temporary.0, destination).map_err(|e| write_error(requested, e))?;
    temporary.0.clear();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "op-figma-save-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn contents(&self) -> Vec<PathBuf> {
            std::fs::read_dir(&self.0)
                .unwrap()
                .map(|v| v.unwrap().path())
                .collect()
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn streaming_failure_preserves_existing_destination_and_removes_temporary_file() {
        let directory = Directory::new();
        let destination = directory.0.join("existing.op");
        std::fs::write(&destination, b"original").unwrap();
        let name = destination.to_str().unwrap();
        for error in [
            CliError::Payload("injected serialization error".into()),
            CliError::Io(format!("write {name:?}: injected I/O error")),
        ] {
            let result = write_atomically::<()>(name, |writer| {
                writer.write_all(b"partial document").unwrap();
                Err(error.clone())
            });
            assert_eq!(result, Err(error));
            assert_eq!(std::fs::read(&destination).unwrap(), b"original");
            assert_eq!(directory.contents(), vec![destination.clone()]);
        }
    }

    #[test]
    fn successful_stream_replaces_existing_file_and_flushes_buffered_bytes() {
        let directory = Directory::new();
        let destination = directory.0.join("existing.op");
        std::fs::write(&destination, b"old").unwrap();
        let value = write_atomically(destination.to_str().unwrap(), |writer| {
            writer
                .write_all(b"new complete document")
                .map_err(|e| write_error("output", e))?;
            Ok(7)
        })
        .unwrap();
        assert_eq!(value, 7);
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"new complete document"
        );
        assert_eq!(directory.contents(), vec![destination]);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replacement_preserves_existing_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let directory = Directory::new();
        let destination = directory.0.join("private.op");
        std::fs::write(&destination, b"old").unwrap();
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_atomically(destination.to_str().unwrap(), |writer| {
            writer
                .write_all(b"new")
                .map_err(|e| write_error("output", e))
        })
        .unwrap();
        assert_eq!(
            std::fs::metadata(destination).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn failed_rename_keeps_destination_and_cleans_sibling() {
        let directory = Directory::new();
        let destination = directory.0.join("destination.op");
        std::fs::create_dir(&destination).unwrap();
        let name = destination.to_str().unwrap();
        let error = write_atomically(name, |writer| {
            writer
                .write_all(b"complete JSON")
                .map_err(|e| write_error(name, e))
        })
        .unwrap_err();
        assert!(matches!(error, CliError::Io(ref message) if message.contains(name)));
        assert!(destination.is_dir());
        assert_eq!(directory.contents(), vec![destination]);
    }
}
