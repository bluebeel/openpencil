//! Process-local canonical file ownership. No document/hub lock is held while
//! a primary claims/drains a resident path. Data/schema stay with their owner.
use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock, Weak},
};
#[path = "file_ownership_io.rs"]
mod io;
pub(crate) use io::{authorize_save, FileVersion, OperationScope, SourceIdentity};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Resident,
    Primary,
    Transient,
}
struct Identity {
    kind: Kind,
}
#[derive(Clone)]
pub(crate) struct Owner(Arc<Identity>);
impl Owner {
    pub(crate) fn resident() -> Self {
        Self(Arc::new(Identity {
            kind: Kind::Resident,
        }))
    }
    pub(crate) fn primary() -> Self {
        Self(Arc::new(Identity {
            kind: Kind::Primary,
        }))
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClaimError {
    Busy,
    PrimaryBound,
    Stale,
    Exhausted,
    Capacity,
    Io(String),
    SourceChanged,
    Unscoped,
}
impl fmt::Display for ClaimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Busy=>"canonical file authority is busy; no new operation was applied",
            Self::PrimaryBound=>"canonical file is bound to another primary authority; unbind it before taking ownership",
            Self::Stale=>"canonical file ownership changed while this request waited; no operation was applied",
            Self::Exhausted=>"canonical file ownership epoch is exhausted; restart the service",
            Self::Capacity=>"canonical file ownership registry is full",
            Self::Io(message)=>return f.write_str(message),
            Self::SourceChanged=>"authoritative file changed externally; reload it before saving",
            Self::Unscoped=>"save destination does not match the current authoritative file operation",
        })
    }
}
impl std::error::Error for ClaimError {}
#[derive(Default)]
struct State {
    owner: Weak<Identity>,
    kind: Option<Kind>,
    active: usize,
    primary_holds: usize,
    epoch: u64,
    exhausted: bool,
    switching: Weak<()>,
}
struct Claim {
    path: PathBuf,
    state: Mutex<State>,
    drained: Condvar,
}
#[derive(Clone)]
pub(crate) struct Handle(Arc<Claim>);
const MAX_PATHS: usize = 16_384;
static CLAIMS: OnceLock<Mutex<HashMap<PathBuf, Weak<Claim>>>> = OnceLock::new();
pub(crate) fn resolve(path: &Path) -> Result<Handle, ClaimError> {
    let path = super::resident_documents::canonical_key(path)
        .map_err(|e| ClaimError::Io(e.to_string()))?;
    let mut claims = CLAIMS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some(existing) = claims.get(&path).and_then(Weak::upgrade) {
        return Ok(Handle(existing));
    }
    if claims.len() >= MAX_PATHS || claims.len() % 64 == 0 {
        claims.retain(|_, claim| claim.strong_count() != 0);
    }
    if claims.len() >= MAX_PATHS {
        return Err(ClaimError::Capacity);
    }
    let claim = Arc::new(Claim {
        path: path.clone(),
        state: Mutex::new(State::default()),
        drained: Condvar::new(),
    });
    claims.insert(path, Arc::downgrade(&claim));
    Ok(Handle(claim))
}
fn same(state: &State, owner: &Owner) -> bool {
    state.owner.ptr_eq(&Arc::downgrade(&owner.0))
}
fn bump(state: &mut State) -> Result<(), ClaimError> {
    let Some(next) = state.epoch.checked_add(1) else {
        state.exhausted = true;
        return Err(ClaimError::Exhausted);
    };
    state.epoch = next;
    Ok(())
}
fn refresh(state: &mut State) -> Result<(), ClaimError> {
    if state.exhausted {
        return Err(ClaimError::Exhausted);
    }
    if state.kind.is_some() && state.owner.strong_count() == 0 {
        if state.active != 0 || state.primary_holds != 0 {
            return Err(ClaimError::Busy);
        }
        state.owner = Weak::new();
        state.kind = None;
        bump(state)?;
    }
    Ok(())
}
fn clear(state: &mut State) {
    state.owner = Weak::new();
    state.kind = None;
    let _ = bump(state);
}

pub(crate) struct BoundClaim {
    handle: Handle,
    owner: Owner,
    epoch: u64,
}
impl BoundClaim {
    pub(crate) fn path(&self) -> &Path {
        self.handle.path()
    }
    pub(crate) fn operation(&self) -> Result<Operation, ClaimError> {
        let mut state = self
            .handle
            .0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        refresh(&mut state)?;
        if !same(&state, &self.owner) || state.epoch != self.epoch || state.primary_holds == 0 {
            return Err(ClaimError::Stale);
        }
        state.active += 1;
        Ok(Operation {
            handle: self.handle.clone(),
            owner: self.owner.clone(),
            epoch: self.epoch,
        })
    }
}
impl Drop for BoundClaim {
    fn drop(&mut self) {
        let mut state = self
            .handle
            .0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if same(&state, &self.owner) && state.epoch == self.epoch {
            state.primary_holds -= 1;
            if state.primary_holds == 0 && state.active == 0 {
                clear(&mut state);
            }
            self.handle.0.drained.notify_all();
        }
    }
}
pub(crate) struct Operation {
    handle: Handle,
    owner: Owner,
    epoch: u64,
}
impl Operation {
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }
    pub(crate) fn path(&self) -> &Path {
        self.handle.path()
    }
    fn check(&self) -> Result<(), ClaimError> {
        let mut state = self
            .handle
            .0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        refresh(&mut state)?;
        if !same(&state, &self.owner) || state.epoch != self.epoch || state.active == 0 {
            return Err(ClaimError::Stale);
        }
        // A pending primary handoff blocks NEW operations, but this nested
        // save must finish the already pinned operation that it is draining.
        Ok(())
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        let mut state = self
            .handle
            .0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if same(&state, &self.owner) && state.epoch == self.epoch {
            state.active -= 1;
            if state.active == 0 {
                if state.kind == Some(Kind::Transient)
                    || (state.kind == Some(Kind::Primary) && state.primary_holds == 0)
                {
                    clear(&mut state);
                }
                self.handle.0.drained.notify_all();
            }
        }
    }
}
struct Switching {
    handle: Handle,
    identity: Arc<()>,
}
impl Drop for Switching {
    fn drop(&mut self) {
        let mut state = self
            .handle
            .0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if state.switching.ptr_eq(&Arc::downgrade(&self.identity)) {
            state.switching = Weak::new();
            self.handle.0.drained.notify_all();
        }
    }
}
impl Handle {
    pub(crate) fn path(&self) -> &Path {
        &self.0.path
    }
    /// Reserve a resident owner without pinning document work. Queue receipts
    /// retain this epoch so pre-handoff commands cannot reach a new owner.
    pub(crate) fn resident_receipt(&self, owner: &Owner) -> Result<u64, ClaimError> {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        self.resident_owner(&mut state, owner, None)?;
        Ok(state.epoch)
    }
    fn resident_owner(
        &self,
        state: &mut State,
        owner: &Owner,
        expected: Option<u64>,
    ) -> Result<(), ClaimError> {
        refresh(state)?;
        if owner.0.kind != Kind::Resident {
            return Err(ClaimError::Stale);
        }
        if state.switching.strong_count() != 0 {
            return Err(ClaimError::Busy);
        }
        if let Some(expected) = expected {
            if state.epoch != expected || !same(state, owner) {
                return Err(ClaimError::Stale);
            }
        }
        if same(state, owner) && state.kind == Some(Kind::Resident) {
            return Ok(());
        }
        if state.kind == Some(Kind::Primary) {
            return Err(ClaimError::PrimaryBound);
        }
        if state.kind.is_some() || state.active != 0 {
            return Err(ClaimError::Busy);
        }
        bump(state)?;
        state.owner = Arc::downgrade(&owner.0);
        state.kind = Some(Kind::Resident);
        Ok(())
    }
    pub(crate) fn resident_operation(
        &self,
        owner: &Owner,
        expected: Option<u64>,
    ) -> Result<Operation, ClaimError> {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        self.resident_owner(&mut state, owner, expected)?;
        state.active += 1;
        Ok(Operation {
            handle: self.clone(),
            owner: owner.clone(),
            epoch: state.epoch,
        })
    }
    /// Destination first, session writer gate second, document lock last.
    /// `wait=false` is the safe compatibility path for callers already holding
    /// mutable state: they refuse rather than wait through another document.
    pub(crate) fn primary(&self, owner: &Owner, wait: bool) -> Result<BoundClaim, ClaimError> {
        self.primary_mode(owner, wait, false)
    }
    pub(crate) fn primary_if_unowned(&self, owner: &Owner) -> Result<BoundClaim, ClaimError> {
        self.primary_mode(owner, false, true)
    }
    fn primary_mode(
        &self,
        owner: &Owner,
        wait: bool,
        fresh: bool,
    ) -> Result<BoundClaim, ClaimError> {
        if owner.0.kind != Kind::Primary {
            return Err(ClaimError::Stale);
        }
        let switching = Switching {
            handle: self.clone(),
            identity: Arc::new(()),
        };
        let result = (|| {
            let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
            refresh(&mut state)?;
            if fresh && state.kind.is_some() && !same(&state, owner) {
                return Err(ClaimError::Busy);
            }
            if state.switching.strong_count() != 0 {
                return Err(ClaimError::Busy);
            }
            if same(&state, owner) && state.kind == Some(Kind::Primary) {
                state.primary_holds += 1;
                return Ok(BoundClaim {
                    handle: self.clone(),
                    owner: owner.clone(),
                    epoch: state.epoch,
                });
            }
            if state.kind == Some(Kind::Primary) && state.primary_holds != 0 {
                return Err(ClaimError::PrimaryBound);
            }
            if !wait && state.active != 0 {
                return Err(ClaimError::Busy);
            }
            state.switching = Arc::downgrade(&switching.identity);
            while state.active != 0 {
                state = self
                    .0
                    .drained
                    .wait(state)
                    .unwrap_or_else(|p| p.into_inner());
                refresh(&mut state)?;
            }
            bump(&mut state)?;
            state.owner = Arc::downgrade(&owner.0);
            state.kind = Some(Kind::Primary);
            state.primary_holds = 1;
            state.switching = Weak::new();
            Ok(BoundClaim {
                handle: self.clone(),
                owner: owner.clone(),
                epoch: state.epoch,
            })
        })();
        drop(switching);
        result
    }
    fn transient(&self) -> Result<Operation, ClaimError> {
        let owner = Owner(Arc::new(Identity {
            kind: Kind::Transient,
        }));
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        refresh(&mut state)?;
        if state.switching.strong_count() != 0 || state.active != 0 {
            return Err(ClaimError::Busy);
        }
        if state.kind == Some(Kind::Primary) {
            return Err(ClaimError::PrimaryBound);
        }
        if state.kind.is_some() {
            return Err(ClaimError::Busy);
        }
        bump(&mut state)?;
        state.owner = Arc::downgrade(&owner.0);
        state.kind = Some(Kind::Transient);
        state.primary_holds = 0;
        state.active = 1;
        Ok(Operation {
            handle: self.clone(),
            owner,
            epoch: state.epoch,
        })
    }
}
