//! Binding ownership independent of the document mutex. Claim destination
//! first, drain session readers second, then load/install under the writer lock.
use crate::{
    document_recovery::DocumentRecovery,
    mcp_serve::file_ownership::{
        self, BoundClaim, ClaimError, OperationScope, Owner, SourceIdentity,
    },
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
};

struct Binding {
    raw: PathBuf,
    claim: BoundClaim,
    source: Arc<SourceIdentity>,
}
#[derive(Default)]
struct State {
    binding: Option<Arc<Binding>>,
    revision: u64,
    readers: usize,
    writer: bool,
    waiting: usize,
    unverified: bool,
}
pub(super) struct PrimaryFiles {
    owner: Owner,
    state: Mutex<State>,
    ready: Condvar,
}
impl PrimaryFiles {
    pub(super) fn unbound() -> Arc<Self> {
        Arc::new(Self {
            owner: Owner::primary(),
            state: Mutex::new(State::default()),
            ready: Condvar::new(),
        })
    }
    /// Compatibility constructors already own their editor. They may claim a
    /// fresh path, but never silently supersede a live resident/primary owner.
    pub(super) fn supplied_editor(path: Option<&Path>) -> Arc<Self> {
        let owner = Self::unbound();
        if let Some(path) = path {
            let binding = (|| {
                let handle = file_ownership::resolve(path)?;
                let claim = handle.primary_if_unowned(&owner.owner)?;
                let source = SourceIdentity::capture(claim.path())?;
                Ok::<_, ClaimError>(Arc::new(Binding {
                    raw: path.to_path_buf(),
                    claim,
                    source,
                }))
            })();
            let mut state = owner.state.lock().unwrap_or_else(|p| p.into_inner());
            match binding {
                Ok(binding) => state.binding = Some(binding),
                Err(_) => state.unverified = true,
            }
        }
        owner
    }
    pub(super) fn verified(&self) -> bool {
        !self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .unverified
    }
    /// The private binding owner is authoritative; routed files never need to
    /// wait for a primary whole-document save just to copy this small header.
    pub(super) fn route(&self) -> (Option<PathBuf>, PathBuf) {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        match &state.binding {
            Some(binding) => (
                Some(binding.raw.clone()),
                binding.claim.path().to_path_buf(),
            ),
            None => (None, PathBuf::from("live://canvas/current")),
        }
    }
    pub(super) fn read(
        self: &Arc<Self>,
        recovery: DocumentRecovery,
    ) -> Result<ReadUse, ClaimError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        while state.writer || state.waiting != 0 {
            state = self.ready.wait(state).unwrap_or_else(|p| p.into_inner());
        }
        if state.unverified {
            return Err(ClaimError::Stale);
        }
        let binding = state.binding.clone();
        state.readers += 1;
        drop(state);
        let reader = Reader {
            owner: Arc::clone(self),
        };
        let io = match &binding {
            Some(binding) => Some(OperationScope::enter(
                Arc::new(binding.claim.operation()?),
                Some(Arc::clone(&binding.source)),
                recovery,
            )),
            None => None,
        };
        Ok(ReadUse {
            _io: io,
            _reader: reader,
            _binding: binding,
        })
    }
    /// Must be called before taking the document or resident hub mutex. New
    /// resident work is refused while this destination drains. No session
    /// writer is held yet, so an active copy can finish its primary callback.
    pub(super) fn prepare(
        self: &Arc<Self>,
        path: Option<&Path>,
        wait: bool,
    ) -> Result<Arc<Prepared>, ClaimError> {
        let revision = self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .revision;
        let claimed = match path {
            Some(path) => {
                let handle = file_ownership::resolve(path)?;
                Some((path.to_path_buf(), handle.primary(&self.owner, wait)?))
            }
            None => None,
        };
        let writer = self.write(wait)?;
        if self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .revision
            != revision
        {
            return Err(ClaimError::Stale);
        }
        // An existing same-primary save may finish while the writer gate drains.
        // Capture its final source identity AFTER that drain, before loading.
        let binding = match claimed {
            Some((raw, claim)) => {
                let source = SourceIdentity::capture(claim.path())?;
                Some(Arc::new(Binding { raw, claim, source }))
            }
            None => None,
        };
        Ok(Arc::new(Prepared {
            owner: Arc::clone(self),
            binding: Mutex::new(binding),
            installed: std::sync::atomic::AtomicBool::new(false),
            _writer: writer,
        }))
    }
    fn write(self: &Arc<Self>, wait: bool) -> Result<Writer, ClaimError> {
        let waiting = Waiting {
            owner: Arc::clone(self),
            armed: std::sync::atomic::AtomicBool::new(false),
        };
        let result = (|| {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if !wait && (state.writer || state.readers != 0 || state.waiting != 0) {
                return Err(ClaimError::Busy);
            }
            state.waiting += 1;
            waiting
                .armed
                .store(true, std::sync::atomic::Ordering::Relaxed);
            while state.writer || state.readers != 0 {
                state = self.ready.wait(state).unwrap_or_else(|p| p.into_inner());
            }
            state.waiting -= 1;
            waiting
                .armed
                .store(false, std::sync::atomic::Ordering::Relaxed);
            state.writer = true;
            Ok(Writer {
                owner: Arc::clone(self),
            })
        })();
        drop(waiting);
        result
    }
}
struct Waiting {
    owner: Arc<PrimaryFiles>,
    armed: std::sync::atomic::AtomicBool,
}
impl Drop for Waiting {
    fn drop(&mut self) {
        if self.armed.load(std::sync::atomic::Ordering::Relaxed) {
            let mut state = self.owner.state.lock().unwrap_or_else(|p| p.into_inner());
            state.waiting -= 1;
            self.owner.ready.notify_all();
        }
    }
}
struct Reader {
    owner: Arc<PrimaryFiles>,
}
impl Drop for Reader {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap_or_else(|p| p.into_inner());
        state.readers -= 1;
        if state.readers == 0 {
            self.owner.ready.notify_all();
        }
    }
}
struct Writer {
    owner: Arc<PrimaryFiles>,
}
impl Drop for Writer {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap_or_else(|p| p.into_inner());
        state.writer = false;
        self.owner.ready.notify_all();
    }
}
pub(super) struct ReadUse {
    _io: Option<OperationScope>,
    _reader: Reader,
    _binding: Option<Arc<Binding>>,
}
pub(super) struct Prepared {
    owner: Arc<PrimaryFiles>,
    binding: Mutex<Option<Arc<Binding>>>,
    installed: std::sync::atomic::AtomicBool,
    _writer: Writer,
}
impl Prepared {
    pub(super) fn load_editor(
        &self,
        raw: &Path,
    ) -> Result<op_editor_core::EditorState, crate::doc_io::DocIoError> {
        let canonical = self
            .path()
            .ok_or_else(|| crate::doc_io::DocIoError::Io("owned load has no bound path".into()))?;
        crate::doc_io::load_editor_state_resolved(&canonical, raw, op_editor_core::Locale::EnUs)
    }
    pub(super) fn owner_is(&self, owner: &Arc<PrimaryFiles>) -> bool {
        Arc::ptr_eq(&self.owner, owner)
    }
    pub(super) fn path(&self) -> Option<PathBuf> {
        self.binding
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|binding| binding.claim.path().to_path_buf())
    }
    pub(super) fn matches(&self, path: Option<&Path>) -> bool {
        match (self.path(), path) {
            (None, None) => true,
            (Some(a), Some(b)) => file_ownership::resolve(b).is_ok_and(|key| a == key.path()),
            _ => false,
        }
    }
    pub(super) fn verify_source(&self) -> Result<(), ClaimError> {
        if let Some(binding) = self
            .binding
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            binding.source.verify()?;
        }
        Ok(())
    }
    /// Called after successful full authoritative load, still under the state
    /// writer mutex. No previous data or bound claim is dropped before success.
    pub(super) fn install(&self) -> Result<(), ClaimError> {
        use std::sync::atomic::Ordering;
        if self.installed.swap(true, Ordering::AcqRel) {
            return Err(ClaimError::Stale);
        }
        self.verify_source()?;
        let next = self
            .binding
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        let mut state = self.owner.state.lock().unwrap_or_else(|p| p.into_inner());
        let revision = state.revision.checked_add(1).ok_or(ClaimError::Exhausted)?;
        let old = std::mem::replace(&mut state.binding, next);
        state.revision = revision;
        state.unverified = false;
        drop(state);
        drop(old);
        Ok(())
    }
}
