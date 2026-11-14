//! Reuse a bounded save buffer on persistent connection/save workers.
//! Only a clean, flushed buffer is retained; no file handle survives the save.

use super::DocIoError;
use std::{
    cell::RefCell,
    fs::File,
    io::{self, BufWriter, Write},
};

const BUFFER_BYTES: usize = 1024 * 1024;

pub(super) struct SaveTarget(Option<File>);

impl Write for SaveTarget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .as_mut()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?
            .write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.0 {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

thread_local! {
    static BUFFER: RefCell<Option<BufWriter<SaveTarget>>> = const { RefCell::new(None) };
}

pub(super) fn with_buffered_file<T>(
    file: File,
    action: impl FnOnce(&mut BufWriter<SaveTarget>) -> Result<T, DocIoError>,
) -> Result<T, DocIoError> {
    let mut writer = BUFFER
        .with(|slot| slot.borrow_mut().take())
        .unwrap_or_else(|| BufWriter::with_capacity(BUFFER_BYTES, SaveTarget(None)));
    debug_assert!(writer.buffer().is_empty());
    debug_assert!(writer.get_ref().0.is_none());
    writer.get_mut().0 = Some(file);
    // A serialization, flush, or panic failure drops the entire writer. Its
    // partial bytes can never be replayed into another document's next save.
    let value = action(&mut writer)?;
    writer
        .flush()
        .map_err(|error| DocIoError::Io(error.to_string()))?;
    drop(writer.get_mut().0.take());
    BUFFER.with(|slot| *slot.borrow_mut() = Some(writer));
    Ok(value)
}

/// Managed save leaders can move between hundreds of connection workers.
/// Share a small pool instead of retaining one MiB per worker's TLS lifetime.
/// Only clean/flushed buffers without file handles ever enter the pool.
pub(super) fn with_shared_buffered_file<T>(
    file: File,
    action: impl FnOnce(&mut BufWriter<SaveTarget>) -> Result<T, DocIoError>,
) -> Result<T, DocIoError> {
    use std::sync::{Mutex, OnceLock};
    const MAX_IDLE_BUFFERS: usize = 8;
    static POOL: OnceLock<Mutex<Vec<BufWriter<SaveTarget>>>> = OnceLock::new();
    let pool = POOL.get_or_init(|| Mutex::new(Vec::new()));
    let reusable = { pool.lock().unwrap_or_else(|p| p.into_inner()).pop() };
    let mut writer =
        reusable.unwrap_or_else(|| BufWriter::with_capacity(BUFFER_BYTES, SaveTarget(None)));
    debug_assert!(writer.buffer().is_empty());
    debug_assert!(writer.get_ref().0.is_none());
    writer.get_mut().0 = Some(file);
    let value = action(&mut writer)?;
    writer
        .flush()
        .map_err(|error| DocIoError::Io(error.to_string()))?;
    drop(writer.get_mut().0.take());
    let mut pool = pool.lock().unwrap_or_else(|p| p.into_inner());
    if pool.len() < MAX_IDLE_BUFFERS {
        pool.push(writer);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Files(PathBuf);
    impl Files {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "op-save-buffer-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            BUFFER.with(|slot| *slot.borrow_mut() = None);
            Self(path)
        }
        fn file(&self, name: &str) -> File {
            File::create(self.0.join(name)).unwrap()
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            BUFFER.with(|slot| *slot.borrow_mut() = None);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn successful_saves_reuse_storage_without_retaining_a_file_or_bytes() {
        let files = Files::new();
        let large = vec![b'x'; BUFFER_BYTES + 17];
        with_buffered_file(files.file("large"), |writer| {
            writer.write_all(&large).unwrap();
            Ok(())
        })
        .unwrap();
        let address = BUFFER.with(|slot| {
            let slot = slot.borrow();
            let writer = slot.as_ref().unwrap();
            assert!(writer.get_ref().0.is_none());
            assert!(writer.buffer().is_empty());
            writer.buffer().as_ptr() as usize
        });
        with_buffered_file(files.file("small"), |writer| {
            writer.write_all(b"different file").unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read(files.0.join("large")).unwrap(), large);
        assert_eq!(
            std::fs::read(files.0.join("small")).unwrap(),
            b"different file"
        );
        BUFFER.with(|slot| {
            let slot = slot.borrow();
            let writer = slot.as_ref().unwrap();
            assert_eq!(writer.buffer().as_ptr() as usize, address);
            assert!(writer.get_ref().0.is_none());
            assert!(writer.buffer().is_empty());
        });
    }

    #[test]
    fn failed_or_panicking_writes_cannot_leak_into_the_next_document() {
        let files = Files::new();
        let result = with_buffered_file(files.file("failed"), |writer| {
            writer.write_all(b"uncommitted secret").unwrap();
            Err::<(), _>(DocIoError::Io("serialization failed".into()))
        });
        assert!(result.is_err());
        BUFFER.with(|slot| assert!(slot.borrow().is_none()));
        // Buffered writes succeed first; a read-only descriptor fails only
        // when the final flush reaches the file.
        let readonly = File::open(files.0.join("failed")).unwrap();
        let flush = with_buffered_file(readonly, |writer| {
            writer.write_all(b"flush must fail").unwrap();
            Ok(())
        });
        assert!(flush.is_err());
        BUFFER.with(|slot| assert!(slot.borrow().is_none()));
        let panic = std::panic::catch_unwind(|| {
            let _ = with_buffered_file(files.file("panic"), |writer| {
                writer.write_all(b"other uncommitted bytes").unwrap();
                panic!("serializer panic");
                #[allow(unreachable_code)]
                Ok(())
            });
        });
        assert!(panic.is_err());
        BUFFER.with(|slot| assert!(slot.borrow().is_none()));
        with_buffered_file(files.file("next"), |writer| {
            writer.write_all(b"clean").unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read(files.0.join("next")).unwrap(), b"clean");
    }
}
