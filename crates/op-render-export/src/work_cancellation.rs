//! Cancellation wakes only registered FIFO tickets; no poller or helper thread.
use super::Inner;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, Weak,
};

type Key = (usize, u64);
struct Listener {
    gate: Weak<Inner>,
    ticket: u64,
}
#[derive(Default)]
struct State {
    cancelled: AtomicBool,
    listeners: Mutex<HashMap<Key, Listener>>,
}
/// A terminal cancellation signal, safe to clone into transport/shutdown owners.
/// Cancelling pending work removes its cheap FIFO record. Granted work retains
/// its lease until its owner drops it; cancellation never reclaims live pixels.
#[derive(Clone, Default)]
pub struct WorkCancellation {
    state: Arc<State>,
}
impl WorkCancellation {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        if self.state.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        let listeners = {
            let mut listeners = self
                .state
                .listeners
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut *listeners)
        };
        // Never hold the token lock while taking a gate lock. In particular,
        // registration can run under a gate lock without cancellation racing
        // between its flag check and Condvar::wait losing the only wakeup.
        for listener in listeners.into_values() {
            if let Some(gate) = listener.gate.upgrade() {
                gate.cancel(listener.ticket);
            }
        }
    }
    pub(super) fn register(&self, gate: &Arc<Inner>, ticket: u64) -> Registration {
        let key = (Arc::as_ptr(gate) as usize, ticket);
        self.state
            .listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                key,
                Listener {
                    gate: Arc::downgrade(gate),
                    ticket,
                },
            );
        Registration {
            state: self.state.clone(),
            key,
        }
    }
}
pub(super) struct Registration {
    state: Arc<State>,
    key: Key,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.state
            .listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.key);
    }
}
