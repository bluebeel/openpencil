//! Thread-local permission spans carry only owned claim/source metadata. They
//! never borrow an editor or cross into deferred export/SSE delivery tasks.
use super::{resolve, ClaimError, Operation};
use crate::document_recovery::DocumentRecovery;
use std::{
    cell::RefCell,
    marker::PhantomData,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileVersion {
    pub(crate) len: u64,
    modified: Option<std::time::SystemTime>,
    identity: (u64, u64),
}
impl FileVersion {
    pub(crate) fn metadata(metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        };
        #[cfg(not(unix))]
        let identity = (0, 0);
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            identity,
        }
    }
    fn read(path: &Path) -> Result<Option<Self>, ClaimError> {
        match std::fs::metadata(path) {
            Ok(metadata) => Ok(Some(Self::metadata(&metadata))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ClaimError::Io(format!("stat {}: {error}", path.display()))),
        }
    }
}
pub(crate) struct SourceIdentity {
    path: PathBuf,
    expected: std::sync::Mutex<Option<FileVersion>>,
}
impl SourceIdentity {
    pub(crate) fn capture(path: &Path) -> Result<Arc<Self>, ClaimError> {
        Ok(Arc::new(Self {
            path: path.to_path_buf(),
            expected: std::sync::Mutex::new(FileVersion::read(path)?),
        }))
    }
    pub(crate) fn from_version(path: &Path, version: FileVersion) -> Arc<Self> {
        Arc::new(Self {
            path: path.to_path_buf(),
            expected: std::sync::Mutex::new(Some(version)),
        })
    }
    pub(crate) fn verify(&self) -> Result<(), ClaimError> {
        let current = FileVersion::read(&self.path)?;
        if current == *self.expected.lock().unwrap_or_else(|p| p.into_inner()) {
            Ok(())
        } else {
            Err(ClaimError::SourceChanged)
        }
    }
}
#[derive(Clone)]
struct Permission {
    operation: Arc<Operation>,
    source: Option<Arc<SourceIdentity>>,
    recovery: DocumentRecovery,
}
thread_local! {static CURRENT:RefCell<Vec<(u64,Permission)>>=const{RefCell::new(Vec::new())};}
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
pub(crate) struct OperationScope {
    id: u64,
    _thread: PhantomData<Rc<()>>,
}
impl OperationScope {
    pub(crate) fn enter(
        operation: Arc<Operation>,
        source: Option<Arc<SourceIdentity>>,
        recovery: DocumentRecovery,
    ) -> Self {
        use std::sync::atomic::Ordering;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        CURRENT.with(|current| {
            current.borrow_mut().push((
                id,
                Permission {
                    operation,
                    source,
                    recovery,
                },
            ))
        });
        Self {
            id,
            _thread: PhantomData,
        }
    }
}
impl Drop for OperationScope {
    fn drop(&mut self) {
        CURRENT.with(|current| {
            let mut current = current.borrow_mut();
            if let Some(index) = current.iter().position(|(id, _)| *id == self.id) {
                current.remove(index);
            }
        });
    }
}
pub(crate) struct SaveAuthorization {
    permission: Permission,
    written: Option<FileVersion>,
}
impl SaveAuthorization {
    pub(crate) fn path(&self) -> &Path {
        self.permission.operation.path()
    }
    pub(crate) fn scope(&self) -> OperationScope {
        OperationScope::enter(
            Arc::clone(&self.permission.operation),
            self.permission.source.clone(),
            self.permission.recovery.clone(),
        )
    }
    pub(crate) fn staging_ready(&mut self, path: &Path) -> Result<(), ClaimError> {
        self.written = FileVersion::read(path)?;
        if self.written.is_none() {
            return Err(ClaimError::Io("completed staging file disappeared".into()));
        }
        Ok(())
    }
    pub(crate) fn committed(mut self) -> Result<(), ClaimError> {
        let result = (|| {
            let written = self.written.take().ok_or_else(|| {
                ClaimError::Io("save did not record its completed staging identity".into())
            })?;
            let actual = FileVersion::read(self.permission.operation.path())?;
            #[cfg(unix)]
            let expected = {
                if actual.as_ref() != Some(&written) {
                    return Err(ClaimError::SourceChanged);
                }
                written
            };
            // Unix rename preserves the completed staging identity. Other
            // replace APIs may merge metadata; retain their established write
            // behavior while recording the resulting source for future checks.
            #[cfg(not(unix))]
            let expected = {
                let _ = written;
                actual.ok_or(ClaimError::SourceChanged)?
            };
            if let Some(source) = &self.permission.source {
                *source.expected.lock().unwrap_or_else(|p| p.into_inner()) = Some(expected);
            }
            Ok(())
        })();
        if result.is_err() {
            self.permission.recovery.trip();
        }
        result
    }
}
pub(crate) fn authorize_save(path: &Path) -> Result<SaveAuthorization, ClaimError> {
    let handle = resolve(path)?;
    let (permission, scoped, last_recovery) = CURRENT.with(|current| {
        let current = current.borrow();
        (
            current
                .iter()
                .rev()
                .find(|(_, p)| p.operation.path() == handle.path())
                .map(|(_, p)| p.clone()),
            !current.is_empty(),
            current.last().map(|(_, p)| p.recovery.clone()),
        )
    });
    let permission = match permission {
        Some(permission) => permission,
        None if scoped => {
            if let Some(recovery) = last_recovery {
                recovery.trip();
            }
            return Err(ClaimError::Unscoped);
        }
        None => {
            let operation = Arc::new(handle.transient()?);
            Permission {
                operation,
                source: None,
                recovery: DocumentRecovery::default(),
            }
        }
    };
    if let Some(source) = &permission.source {
        if let Err(error) = source.verify() {
            permission.recovery.trip();
            return Err(error);
        }
    }
    permission.operation.check()?;
    Ok(SaveAuthorization {
        permission,
        written: None,
    })
}
