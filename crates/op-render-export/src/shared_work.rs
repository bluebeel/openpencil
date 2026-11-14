//! FIFO admission before expensive scene/pixel work. Two render leases share
//! capacity; conversion is exclusive, including retained encoded responses.
//! Queue records hold no scenes, pixels, outputs, or renderer threads.

use crate::ExportError;
use std::collections::VecDeque;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

#[path = "work_cancellation.rs"]
mod cancellation;
pub use cancellation::WorkCancellation;

pub(crate) const MAX_RENDER_ADMISSIONS: usize = 2;
const CONVERSION: usize = MAX_RENDER_ADMISSIONS + 1;
pub const DEFAULT_MAX_QUEUED_WORK: usize = 4096;
const MAX_QUEUE_LIMIT: usize = 65_536;

struct Ticket {
    id: u64,
    units: usize,
    ready: Condvar,
}
struct State {
    active: usize,
    queue: VecDeque<Arc<Ticket>>,
    next_id: u64,
    max_queued: usize,
    enqueued: u64,
    cancelled: u64,
    refused: u64,
}
struct Inner {
    state: Mutex<State>,
}
impl State {
    fn fits(&self, units: usize) -> bool {
        if units == CONVERSION {
            self.active == 0
        } else {
            self.active < MAX_RENDER_ADMISSIONS
        }
    }
    fn wake_front(&self) {
        if let Some(ticket) = self.queue.front().filter(|ticket| self.fits(ticket.units)) {
            ticket.ready.notify_one();
        }
    }
    fn remove(&mut self, id: u64) {
        if let Some(at) = self.queue.iter().position(|ticket| ticket.id == id) {
            let ticket = self.queue.remove(at).unwrap();
            self.cancelled = self.cancelled.saturating_add(1);
            ticket.ready.notify_one();
            self.wake_front();
        }
    }
    fn wake_all_on_failure(&self) {
        for ticket in &self.queue {
            ticket.ready.notify_one();
        }
    }
}
impl Inner {
    fn lock(&self) -> Result<MutexGuard<'_, State>, ExportError> {
        self.state.lock().map_err(|poisoned| {
            poisoned.into_inner().wake_all_on_failure();
            ExportError::RenderWorkerUnavailable
        })
    }
    fn cancel(&self, id: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.remove(id);
        if self.state.is_poisoned() {
            state.wake_all_on_failure();
        }
    }
}

/// Lightweight observation; these counts are admission, not process RSS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedWorkStats {
    pub available: bool,
    pub active_renders: usize,
    pub conversion_active: bool,
    pub queued: usize,
    pub max_queued: usize,
    pub enqueued: u64,
    pub cancelled: u64,
    pub refused: u64,
}

/// Cloneable gate for one service's expensive render/conversion work.
#[derive(Clone)]
pub struct SharedWorkGate {
    inner: Arc<Inner>,
}
impl Default for SharedWorkGate {
    fn default() -> Self {
        Self::with_queue_limit(DEFAULT_MAX_QUEUED_WORK)
    }
}

/// Capacity releases on normal return, cancellation, disconnect, or unwind.
#[must_use = "hold this lease through processing and response writing"]
pub struct SharedWorkLease {
    gate: SharedWorkGate,
    units: usize,
}

// Declared before a waiting mutex guard, so unwinding drops that guard first.
// Removing a forgotten/cancelled ticket cannot strand the FIFO behind it.
struct Pending {
    inner: Arc<Inner>,
    id: Option<u64>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            self.inner.cancel(id);
        }
    }
}

impl SharedWorkGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bound cheap pending metadata independently of the physical capacity.
    /// Zero permits only immediate acquisition. Values cap at 65,536 waiters.
    pub fn with_queue_limit(max_queued: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    active: 0,
                    queue: VecDeque::new(),
                    next_id: 0,
                    max_queued: max_queued.min(MAX_QUEUE_LIMIT),
                    enqueued: 0,
                    cancelled: 0,
                    refused: 0,
                }),
            }),
        }
    }
    fn try_units(&self, units: usize) -> Option<SharedWorkLease> {
        let mut state = self.inner.lock().ok()?;
        // A fail-fast caller cannot jump an already-queued conversion/render.
        if !state.queue.is_empty() || !state.fits(units) {
            return None;
        }
        state.active += units;
        Some(SharedWorkLease {
            gate: self.clone(),
            units,
        })
    }
    /// Reserve before deriving a scene. Existing synchronous callers fail fast.
    pub fn try_render(&self) -> Option<SharedWorkLease> {
        self.try_units(1)
    }
    /// Conversion cannot overlap rendering or another conversion.
    pub fn try_conversion(&self) -> Option<SharedWorkLease> {
        self.try_units(CONVERSION)
    }

    /// Wait without polling or starting threads. Call after document locks are
    /// released and before deriving scenes/allocating outputs. Cancellation is
    /// terminal for this token; after a lease is granted it is not revoked.
    pub fn acquire_render(
        &self,
        cancellation: &WorkCancellation,
    ) -> Result<SharedWorkLease, ExportError> {
        self.acquire_units(1, cancellation)
    }
    /// FIFO-exclusive conversion also prevents starvation by later renders.
    pub fn acquire_conversion(
        &self,
        cancellation: &WorkCancellation,
    ) -> Result<SharedWorkLease, ExportError> {
        self.acquire_units(CONVERSION, cancellation)
    }
    fn acquire_units(
        &self,
        units: usize,
        cancellation: &WorkCancellation,
    ) -> Result<SharedWorkLease, ExportError> {
        if cancellation.is_cancelled() {
            return Err(ExportError::WorkAdmissionCancelled);
        }
        let mut pending = Pending {
            inner: self.inner.clone(),
            id: None,
        };
        let mut state = self.inner.lock()?;
        if cancellation.is_cancelled() {
            return Err(ExportError::WorkAdmissionCancelled);
        }
        if state.queue.is_empty() && state.fits(units) {
            state.active += units;
            return Ok(SharedWorkLease {
                gate: self.clone(),
                units,
            });
        }
        if state.queue.len() >= state.max_queued {
            state.refused = state.refused.saturating_add(1);
            return Err(ExportError::WorkAdmissionQueueFull {
                max_queued: state.max_queued,
            });
        }
        let id = state
            .next_id
            .checked_add(1)
            .ok_or(ExportError::RenderWorkerUnavailable)?;
        state.next_id = id;
        let ticket = Arc::new(Ticket {
            id,
            units,
            ready: Condvar::new(),
        });
        state.queue.push_back(ticket.clone());
        pending.id = Some(id);
        state.enqueued = state.enqueued.saturating_add(1);
        // Registration takes no gate lock on Drop. cancel() copies listeners
        // before taking a gate lock, so token/gate locks cannot invert.
        let _registration = cancellation.register(&self.inner, id);
        loop {
            if cancellation.is_cancelled() {
                state.remove(id);
                pending.id = None;
                return Err(ExportError::WorkAdmissionCancelled);
            }
            if state.queue.front().is_some_and(|head| head.id == id) && state.fits(units) {
                state.queue.pop_front();
                state.active += units;
                pending.id = None;
                state.wake_front();
                return Ok(SharedWorkLease {
                    gate: self.clone(),
                    units,
                });
            }
            state = match ticket.ready.wait(state) {
                Ok(state) => state,
                Err(poisoned) => {
                    let mut state = poisoned.into_inner();
                    state.remove(id);
                    pending.id = None;
                    state.wake_all_on_failure();
                    return Err(ExportError::RenderWorkerUnavailable);
                }
            };
        }
    }
    pub fn stats(&self) -> SharedWorkStats {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        SharedWorkStats {
            available: !self.inner.state.is_poisoned(),
            active_renders: if state.active == CONVERSION {
                0
            } else {
                state.active
            },
            conversion_active: state.active == CONVERSION,
            queued: state.queue.len(),
            max_queued: state.max_queued,
            enqueued: state.enqueued,
            cancelled: state.cancelled,
            refused: state.refused,
        }
    }
}
impl Drop for SharedWorkLease {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.active = state.active.saturating_sub(self.units);
        state.wake_front();
        if self.gate.inner.state.is_poisoned() {
            state.wake_all_on_failure();
        }
    }
}

/// Production raster and conversion routes share this process-local gate.
pub fn shared_work_gate() -> &'static SharedWorkGate {
    static GATE: OnceLock<SharedWorkGate> = OnceLock::new();
    GATE.get_or_init(SharedWorkGate::new)
}
#[cfg(test)]
#[path = "shared_work_tests.rs"]
mod tests;
