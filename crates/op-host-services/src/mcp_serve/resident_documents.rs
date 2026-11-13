//! Bounded process-local authorities for alternate filePath documents.
//!
//! One canonical file has one writer lock. Only idle entries can be evicted;
//! every successful document mutation is saved before its MCP acknowledgement.

use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::SystemTime,
};

use op_editor_core::EditorState;

use super::resident_charge::resident_charge;

const RESERVATION: usize = 64 * 1024;
const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AuthorityError {
    Io {
        path: PathBuf,
        operation: &'static str,
        message: String,
    },
    Capacity {
        max_documents: usize,
        max_bytes: usize,
    },
    SourceTooLarge {
        path: PathBuf,
        bytes: u64,
        max_bytes: u64,
    },
    StateOverBudget {
        path: PathBuf,
        max_bytes: usize,
    },
    ExternalChange {
        path: PathBuf,
    },
    BoundPrimaryConflict {
        path: PathBuf,
    },
    Poisoned,
}

impl fmt::Display for AuthorityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, operation, message } => write!(f, "{operation} {}: {message}", path.display()),
            Self::Capacity { max_documents, max_bytes } => write!(f, "document authority busy (limit {max_documents} documents / {max_bytes} accounted bytes); retry later"),
            Self::SourceTooLarge { path, bytes, max_bytes } => write!(f, "{} has {bytes} source bytes; resident authority limit is {max_bytes}", path.display()),
            Self::StateOverBudget { path, max_bytes } => write!(f, "{} exceeds the resident state accounting limit ({max_bytes} bytes)", path.display()),
            Self::ExternalChange { path } => write!(f, "{} changed externally; retry against the new document", path.display()),
            Self::BoundPrimaryConflict { path } => write!(f, "{} is the bound live document; copying onto it requires a live-authority handoff", path.display()),
            Self::Poisoned => f.write_str("document authority is unavailable after a failed request"),
        }
    }
}

impl std::error::Error for AuthorityError {}

fn io(path: &Path, operation: &'static str, error: impl fmt::Display) -> AuthorityError {
    AuthorityError::Io {
        path: path.to_path_buf(),
        operation,
        message: error.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    identity: (u64, u64),
}

impl FileStamp {
    pub fn read(path: &Path) -> Result<Self, AuthorityError> {
        let metadata = std::fs::metadata(path).map_err(|e| io(path, "stat", e))?;
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        };
        #[cfg(not(unix))]
        let identity = (0, 0);
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            identity,
        })
    }

    pub fn verify(&self, path: &Path) -> Result<(), AuthorityError> {
        if &Self::read(path)? == self {
            Ok(())
        } else {
            Err(AuthorityError::ExternalChange {
                path: path.to_path_buf(),
            })
        }
    }
}

#[derive(Default)]
struct Resident {
    editor: Option<EditorState>,
    stamp: Option<FileStamp>,
}

struct Slot {
    path: PathBuf,
    resident: Mutex<Resident>,
    charge: AtomicUsize,
    last_used: AtomicU64,
}

pub(super) struct ResidentDocuments {
    slots: Mutex<HashMap<PathBuf, Arc<Slot>>>,
    cold_load: Mutex<()>,
    tick: AtomicU64,
    max_documents: usize,
    max_bytes: usize,
}

pub(super) struct DocumentLease<'a> {
    hub: &'a ResidentDocuments,
    slot: Arc<Slot>,
}

pub(super) struct ActionResult<T> {
    pub value: T,
    pub persisted: bool,
    pub invalidate: bool,
}

impl Default for ResidentDocuments {
    fn default() -> Self {
        Self::with_limits(8, 128 * 1024 * 1024)
    }
}

impl ResidentDocuments {
    fn with_limits(max_documents: usize, max_bytes: usize) -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
            cold_load: Mutex::new(()),
            tick: AtomicU64::new(0),
            max_documents,
            max_bytes,
        }
    }

    pub fn acquire(&self, path: &Path) -> Result<DocumentLease<'_>, AuthorityError> {
        let path = canonical_key(path)?;
        let mut slots = self.slots.lock().map_err(|_| AuthorityError::Poisoned)?;
        let tick = self.tick.fetch_add(1, Ordering::Relaxed);
        if let Some(slot) = slots.get(&path) {
            slot.last_used.store(tick, Ordering::Relaxed);
            return Ok(DocumentLease {
                hub: self,
                slot: Arc::clone(slot),
            });
        }
        while slots.len() >= self.max_documents
            || total_charge(&slots).saturating_add(RESERVATION) > self.max_bytes
        {
            if !evict_idle(&mut slots) {
                return Err(self.capacity());
            }
        }
        let slot = Arc::new(Slot {
            path: path.clone(),
            resident: Mutex::new(Resident::default()),
            charge: AtomicUsize::new(RESERVATION),
            last_used: AtomicU64::new(tick),
        });
        slots.insert(path, Arc::clone(&slot));
        Ok(DocumentLease { hub: self, slot })
    }

    fn capacity(&self) -> AuthorityError {
        AuthorityError::Capacity {
            max_documents: self.max_documents,
            max_bytes: self.max_bytes,
        }
    }

    fn adopt_charge(&self, slot: &Slot, bytes: usize) -> Result<bool, AuthorityError> {
        let mut slots = self.slots.lock().map_err(|_| AuthorityError::Poisoned)?;
        while total_charge(&slots)
            .saturating_sub(slot.charge.load(Ordering::Relaxed))
            .saturating_add(bytes)
            > self.max_bytes
        {
            if !evict_idle(&mut slots) {
                return Ok(false);
            }
        }
        slot.charge.store(bytes, Ordering::Relaxed);
        Ok(true)
    }

    #[cfg(test)]
    fn stats(&self) -> (usize, usize) {
        let slots = self.slots.lock().unwrap();
        (slots.len(), total_charge(&slots))
    }
}

impl DocumentLease<'_> {
    pub fn path(&self) -> &Path {
        &self.slot.path
    }

    /// Lock a save-copy destination without parsing its previous document.
    /// Copy callers coordinate before locking their source, avoiding cycles.
    pub fn with_destination<T>(
        &self,
        action: impl FnOnce(&Path) -> ActionResult<T>,
    ) -> Result<T, AuthorityError> {
        let mut resident = self
            .slot
            .resident
            .lock()
            .map_err(|_| AuthorityError::Poisoned)?;
        let result = action(self.path());
        if result.persisted || result.invalidate {
            resident.editor = None;
            resident.stamp = None;
            self.slot.charge.store(RESERVATION, Ordering::Relaxed);
        }
        Ok(result.value)
    }

    pub fn with_editor<T>(
        &self,
        create: bool,
        action: impl FnOnce(&mut EditorState, &FileStamp) -> ActionResult<T>,
    ) -> Result<T, AuthorityError> {
        let mut resident = self
            .slot
            .resident
            .lock()
            .map_err(|_| AuthorityError::Poisoned)?;
        if create && !self.path().exists() {
            if let Some(parent) = self.path().parent() {
                std::fs::create_dir_all(parent).map_err(|e| io(parent, "create directory", e))?;
            }
            super::save_editor_state(&EditorState::new(), self.path())
                .map_err(|e| io(self.path(), "create document", e))?;
        }
        let stamp = FileStamp::read(self.path())?;
        if resident.editor.is_none() || resident.stamp.as_ref() != Some(&stamp) {
            // Unknown loaded states never accumulate concurrently. The source
            // cap bounds input bytes; the ledger decides whether to retain it.
            let _loading = self
                .hub
                .cold_load
                .lock()
                .map_err(|_| AuthorityError::Poisoned)?;
            if stamp.len > MAX_SOURCE_BYTES {
                return Err(AuthorityError::SourceTooLarge {
                    path: self.path().to_path_buf(),
                    bytes: stamp.len,
                    max_bytes: MAX_SOURCE_BYTES,
                });
            }
            let editor =
                super::load_editor_state(self.path()).map_err(|e| io(self.path(), "load", e))?;
            stamp.verify(self.path())?;
            let charge = resident_charge(&editor, self.hub.max_bytes).map_err(|_| {
                AuthorityError::StateOverBudget {
                    path: self.path().to_path_buf(),
                    max_bytes: self.hub.max_bytes,
                }
            })?;
            if !self.hub.adopt_charge(&self.slot, charge)? {
                return Err(self.hub.capacity());
            }
            resident.editor = Some(editor);
            resident.stamp = Some(stamp);
        }
        let expected = resident
            .stamp
            .as_ref()
            .cloned()
            .ok_or(AuthorityError::Poisoned)?;
        let editor = resident.editor.as_mut().ok_or(AuthorityError::Poisoned)?;
        let before = accounting_marker(editor);
        let result = action(editor, &expected);
        let changed = before != accounting_marker(editor) || result.persisted;
        let current = FileStamp::read(self.path());
        let keep = !result.invalidate
            && current
                .as_ref()
                .is_ok_and(|stamp| result.persisted || stamp == &expected);
        if keep {
            if !changed {
                return Ok(result.value);
            }
            let charge = resident_charge(resident.editor.as_ref().unwrap(), self.hub.max_bytes);
            if let Ok(charge) = charge {
                if self.hub.adopt_charge(&self.slot, charge)? {
                    resident.stamp = current.ok();
                    return Ok(result.value);
                }
            }
        }
        // Failed commits and unaccountable/over-budget states are never kept.
        // Durable writes remain durable; the next request reloads the file.
        resident.editor = None;
        resident.stamp = None;
        self.slot.charge.store(RESERVATION, Ordering::Relaxed);
        Ok(result.value)
    }
}

fn accounting_marker(state: &EditorState) -> (u64, usize, usize, usize, usize, usize) {
    (
        state.document_revision(),
        state.clipboard.as_ptr() as usize,
        state.clipboard.len(),
        state.clipboard.capacity(),
        state.history.past.len(),
        state.history.future.len(),
    )
}

fn total_charge(slots: &HashMap<PathBuf, Arc<Slot>>) -> usize {
    slots.values().fold(0usize, |sum, slot| {
        sum.saturating_add(slot.charge.load(Ordering::Relaxed))
    })
}

fn evict_idle(slots: &mut HashMap<PathBuf, Arc<Slot>>) -> bool {
    let victim = slots
        .iter()
        .filter(|(_, slot)| Arc::strong_count(slot) == 1)
        .min_by_key(|(_, slot)| slot.last_used.load(Ordering::Relaxed))
        .map(|(path, _)| path.clone());
    victim.is_some_and(|path| slots.remove(&path).is_some())
}

pub(super) fn canonical_key(path: &Path) -> Result<PathBuf, AuthorityError> {
    if path.exists() {
        return path.canonicalize().map_err(|e| io(path, "canonicalize", e));
    }
    let absolute = std::path::absolute(path).map_err(|e| io(path, "resolve path", e))?;
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        suffix.push(
            ancestor
                .file_name()
                .ok_or_else(|| io(path, "resolve path", "no existing parent"))?,
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| io(path, "resolve path", "no existing parent"))?;
    }
    let mut canonical = ancestor
        .canonicalize()
        .map_err(|e| io(path, "canonicalize parent", e))?;
    for part in suffix.into_iter().rev() {
        canonical.push(part);
    }
    Ok(canonical)
}

pub(super) fn resident_documents() -> &'static ResidentDocuments {
    static HUB: OnceLock<ResidentDocuments> = OnceLock::new();
    HUB.get_or_init(ResidentDocuments::default)
}

#[cfg(test)]
#[path = "resident_documents_tests.rs"]
mod tests;
