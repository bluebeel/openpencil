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

use super::resident_charge::{
    resident_charge_after_commands, resident_charge_cached, ChargeCache, DocumentDelta,
};

const RESERVATION: usize = 64 * 1024;
#[path = "resident_limits.rs"]
mod resident_limits;
use resident_limits::ResidentLimits;

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
    Uncertain,
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
            Self::Uncertain => f.write_str(crate::document_recovery::REQUIRED),
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
    recovery: crate::document_recovery::DocumentRecovery,
    editor: Option<EditorState>,
    stamp: Option<FileStamp>,
    charge_cache: ChargeCache,
}

struct Slot {
    path: PathBuf,
    recovery: crate::document_recovery::DocumentRecovery,
    groups: super::resident_groups::Groups,
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
    max_source_bytes: u64,
}

pub(super) struct DocumentLease<'a> {
    hub: &'a ResidentDocuments,
    slot: Arc<Slot>,
}

/// A request-local filesystem resolution, never a persistent symlink cache.
#[derive(Clone)]
pub(super) struct CanonicalPath(PathBuf);
impl CanonicalPath {
    pub(super) fn resolve(path: &Path) -> Result<Self, AuthorityError> {
        canonical_key(path).map(Self)
    }
    pub(super) fn as_path(&self) -> &Path {
        &self.0
    }
}

pub(super) struct ActionResult<T> {
    pub value: T,
    pub persisted: bool,
    pub invalidate: bool,
}

impl Default for ResidentDocuments {
    fn default() -> Self {
        let limits = ResidentLimits::from_environment();
        Self {
            max_source_bytes: limits.source_bytes,
            ..Self::with_limits(8, limits.accounted_bytes)
        }
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
            max_source_bytes: 512 * 1024 * 1024,
        }
    }

    #[cfg(test)]
    pub fn acquire(&self, path: &Path) -> Result<DocumentLease<'_>, AuthorityError> {
        self.acquire_canonical(&CanonicalPath::resolve(path)?)
    }

    pub(super) fn acquire_canonical(
        &self,
        path: &CanonicalPath,
    ) -> Result<DocumentLease<'_>, AuthorityError> {
        let path = &path.0;
        let mut slots = self.slots.lock().map_err(|_| AuthorityError::Poisoned)?;
        let tick = self.tick.fetch_add(1, Ordering::Relaxed);
        if let Some(slot) = slots.get(path) {
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
        let recovery = crate::document_recovery::DocumentRecovery::default();
        let slot = Arc::new(Slot {
            path: path.clone(),
            resident: Mutex::new(Resident {
                recovery: recovery.clone(),
                ..Default::default()
            }),
            recovery,
            groups: Default::default(),
            charge: AtomicUsize::new(RESERVATION),
            last_used: AtomicU64::new(tick),
        });
        slots.insert(path.clone(), Arc::clone(&slot));
        Ok(DocumentLease { hub: self, slot })
    }

    fn capacity(&self) -> AuthorityError {
        AuthorityError::Capacity {
            max_documents: self.max_documents,
            max_bytes: self.max_bytes,
        }
    }

    pub(super) fn group_snapshots(&self) -> Vec<serde_json::Value> {
        let slots = self
            .slots
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        slots.values().map(|slot| slot.groups.snapshot()).collect()
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

    pub(super) fn groups(&self) -> &super::resident_groups::Groups {
        &self.slot.groups
    }
    pub(super) fn recovery(&self) -> crate::document_recovery::DocumentRecovery {
        self.slot.recovery.clone()
    }

    pub(super) fn accounting_limit(&self) -> usize {
        self.hub.max_bytes
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
        if resident.recovery.required() {
            return Err(AuthorityError::Uncertain);
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| action(self.path())));
        if result.is_err() {
            resident.recovery.trip();
        }
        resident.editor = None;
        resident.stamp = None;
        resident.charge_cache = ChargeCache::default();
        self.slot.charge.store(RESERVATION, Ordering::Relaxed);
        let result = result.map_err(|_| AuthorityError::Uncertain)?;
        Ok(result.value)
    }

    #[cfg(test)]
    pub fn with_editor<T>(
        &self,
        create: bool,
        action: impl FnOnce(&mut EditorState, &FileStamp) -> ActionResult<T>,
    ) -> Result<T, AuthorityError> {
        self.with_editor_guarded(create, |editor, stamp, _| action(editor, stamp))
    }

    #[cfg(test)]
    pub fn with_editor_guarded<T>(
        &self,
        create: bool,
        action: impl FnOnce(
            &mut EditorState,
            &FileStamp,
            &crate::document_recovery::DocumentRecovery,
        ) -> ActionResult<T>,
    ) -> Result<T, AuthorityError> {
        self.with_editor_accounted(create, |editor, stamp, recovery| {
            (action(editor, stamp, recovery), None)
        })
    }

    /// Trusted routed dispatcher only: every canonical mutation must pass its
    /// observed capture receipt. Arbitrary/public closures use the full ledger.
    pub(super) fn with_editor_accounted<T>(
        &self,
        create: bool,
        action: impl FnOnce(
            &mut EditorState,
            &FileStamp,
            &crate::document_recovery::DocumentRecovery,
        ) -> (ActionResult<T>, Option<DocumentDelta>),
    ) -> Result<T, AuthorityError> {
        let mut resident = self.slot.resident.lock().unwrap_or_else(|poison| {
            let resident = poison.into_inner();
            resident.recovery.trip();
            resident
        });
        let recovering = resident.recovery.required();
        if recovering {
            resident.editor = None;
            resident.stamp = None;
            resident.charge_cache = ChargeCache::default();
            self.slot.charge.store(RESERVATION, Ordering::Relaxed);
        }
        if create && !self.path().exists() {
            if recovering {
                return Err(io(
                    self.path(),
                    "recover",
                    "authoritative saved file is missing",
                ));
            }
            if let Some(parent) = self.path().parent() {
                std::fs::create_dir_all(parent).map_err(|e| io(parent, "create directory", e))?;
            }
            super::save_editor_state(&EditorState::new(), self.path())
                .map_err(|e| io(self.path(), "create document", e))?;
        }
        let stamp = FileStamp::read(self.path())?;
        if resident.editor.is_none() || resident.stamp.as_ref() != Some(&stamp) {
            // An externally replaced state is no longer authoritative. Drop
            // it before decoding its replacement, including on load failure.
            resident.editor = None;
            resident.stamp = None;
            resident.charge_cache = ChargeCache::default();
            self.slot.charge.store(RESERVATION, Ordering::Relaxed);
            // Unknown loaded states never accumulate concurrently. The source
            // cap bounds input bytes; the ledger decides whether to retain it.
            let _loading = self
                .hub
                .cold_load
                .lock()
                .map_err(|_| AuthorityError::Poisoned)?;
            if stamp.len > self.hub.max_source_bytes {
                return Err(AuthorityError::SourceTooLarge {
                    path: self.path().to_path_buf(),
                    bytes: stamp.len,
                    max_bytes: self.hub.max_source_bytes,
                });
            }
            let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let editor = super::load_editor_state(self.path())
                    .map_err(|e| io(self.path(), "load", e))?;
                stamp.verify(self.path())?;
                let mut charge_cache = ChargeCache::default();
                let charge = resident_charge_cached(&editor, self.hub.max_bytes, &mut charge_cache)
                    .map_err(|_| AuthorityError::StateOverBudget {
                        path: self.path().to_path_buf(),
                        max_bytes: self.hub.max_bytes,
                    })?;
                Ok::<_, AuthorityError>((editor, charge, charge_cache))
            }));
            let (editor, charge, charge_cache) = match loaded {
                Ok(result) => result?,
                Err(_) => {
                    resident.recovery.trip();
                    return Err(AuthorityError::Uncertain);
                }
            };
            if !self.hub.adopt_charge(&self.slot, charge)? {
                return Err(self.hub.capacity());
            }
            resident.editor = Some(editor);
            resident.stamp = Some(stamp);
            resident.charge_cache = charge_cache;
            let epoch = resident.recovery.epoch();
            if resident.recovery.required() && !resident.recovery.recovered(epoch) {
                return Err(AuthorityError::Uncertain);
            }
            self.slot.resident.clear_poison();
        }
        let expected = resident
            .stamp
            .as_ref()
            .cloned()
            .ok_or(AuthorityError::Poisoned)?;
        let fence = resident.recovery.clone();
        let editor = resident.editor.as_mut().ok_or(AuthorityError::Poisoned)?;
        let before = accounting_marker(editor);
        let (result, document_delta) = {
            let operation = fence.watch();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                action(editor, &expected, &fence)
            }));
            match outcome {
                Ok(result) => {
                    operation.complete();
                    result
                }
                Err(_) => {
                    drop(operation);
                    resident.editor = None;
                    resident.stamp = None;
                    resident.charge_cache = ChargeCache::default();
                    self.slot.charge.store(RESERVATION, Ordering::Relaxed);
                    return Err(AuthorityError::Uncertain);
                }
            }
        };
        if fence.required() {
            resident.editor = None;
            resident.stamp = None;
            resident.charge_cache = ChargeCache::default();
            self.slot.charge.store(RESERVATION, Ordering::Relaxed);
            return Err(AuthorityError::Uncertain);
        }
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
            let Resident {
                editor,
                charge_cache,
                ..
            } = &mut *resident;
            let charge = resident_charge_after_commands(
                editor.as_ref().unwrap(),
                self.hub.max_bytes,
                charge_cache,
                document_delta,
            );
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
        resident.charge_cache = ChargeCache::default();
        self.slot.charge.store(RESERVATION, Ordering::Relaxed);
        Ok(result.value)
    }
}

fn accounting_marker(state: &EditorState) -> (u64, u64, usize, usize, usize, usize, usize) {
    (
        state.document_generation(),
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
