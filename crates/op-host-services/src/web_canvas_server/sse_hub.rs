//! Latest-value fan-out for the SSE update stream.
//!
//! Split out of the `web_canvas_server` spine at the 800-line cap. The whole
//! design is one decision — a subscriber gets a slot, not a queue — and the
//! reasoning for it lives on `SseSlot`.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use super::SseTick;

type Subscribers = Mutex<HashMap<usize, Weak<SseSlot>>>;

/// One subscriber's latest-value slot.
///
/// Deliberately NOT a queue. The client re-reads whatever the counters point
/// at, so only the newest tick has any meaning — an older one is not
/// information the client lost, it is information the newer one already
/// contains. A queue therefore buys nothing and costs the one thing that
/// matters: a subscriber that stops reading (a paused tab, a stalled socket,
/// a laptop that slept) accumulates one entry per mutation, unbounded, in a
/// process shared with every other account.
///
/// So the slot holds exactly one tick and a publisher overwrites it. A burst
/// of a thousand mutations against a subscriber that never wakes leaves one
/// tick behind, not a thousand.
pub(crate) struct SseSlot {
    latest: Mutex<Option<SseTick>>,
    ready: Condvar,
    async_ready: tokio::sync::Notify,
    owner: Weak<Subscribers>,
}

impl SseSlot {
    fn new() -> Self {
        Self {
            latest: Mutex::new(None),
            ready: Condvar::new(),
            async_ready: tokio::sync::Notify::new(),
            owner: Weak::new(),
        }
    }

    /// Overwrite the pending tick and wake the waiter.
    fn publish(&self, tick: SseTick) {
        let mut latest = self.latest.lock().unwrap_or_else(|p| p.into_inner());
        if *latest == Some(tick) {
            return;
        }
        *latest = Some(tick);
        drop(latest);
        self.ready.notify_one();
        self.async_ready.notify_one();
    }

    /// Take the pending tick, waiting up to `timeout` for one.
    ///
    /// `None` means the timeout elapsed with nothing published — the caller's
    /// cue to emit a heartbeat, which is also how a disconnected socket is
    /// noticed.
    pub(crate) fn take_latest(&self, timeout: Duration) -> Option<SseTick> {
        let guard = self.latest.lock().unwrap_or_else(|p| p.into_inner());
        let (mut guard, _) = self
            .ready
            .wait_timeout_while(guard, timeout, |latest| latest.is_none())
            .unwrap_or_else(|p| p.into_inner());
        guard.take()
    }

    /// Wait without blocking a request worker. Notify retains one permit when
    /// publication precedes registration, matching the single latest-value slot.
    pub(crate) async fn notified_async(&self) {
        self.async_ready.notified().await;
    }

    /// Consume outside a cancelable wait, after notification or heartbeat wins.
    pub(crate) fn take_pending(&self) -> Option<SseTick> {
        self.latest.lock().unwrap_or_else(|p| p.into_inner()).take()
    }

    #[cfg(test)]
    pub(crate) fn pending(&self) -> Option<SseTick> {
        self.take_pending()
    }
}

impl Drop for SseSlot {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            owner
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&(self as *const Self as usize));
        }
    }
}

/// Broadcast hub for SSE subscribers. Each `GET /api/mcp/events` connection
/// registers a slot; a document mutation publishes the new tick into every
/// one of them, and each SSE socket task writes it to its socket.
///
/// Subscribers are held weakly. A disconnected slot unlinks itself in O(1),
/// including files that never broadcast again. Delivery never holds the registry.
#[derive(Default)]
pub struct SseHub {
    subscribers: Arc<Subscribers>,
    // Keep publication ordering consistent with the old hub-wide lock while
    // allowing subscriptions and disconnections during subscriber delivery.
    fanout: Mutex<()>,
}

impl SseHub {
    /// Register a subscriber. Socket tasks await ticks without a request worker;
    /// generic synchronous callers can block on the same slot. Dropping it unregisters.
    pub(crate) fn subscribe(&self) -> Arc<SseSlot> {
        let mut initial = SseSlot::new();
        initial.owner = Arc::downgrade(&self.subscribers);
        let slot = Arc::new(initial);
        // A live Arc allocation is a unique internal key. Its destructor
        // unlinks before the allocator can reuse this address, even without a
        // later broadcast. No reconnect-time scan or permanent tombstone.
        self.subscribers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(Arc::as_ptr(&slot) as usize, Arc::downgrade(&slot));
        slot
    }

    /// Publish a tick to all live subscribers, pruning any whose connection
    /// ended (their `Arc` was dropped).
    pub(crate) fn broadcast(&self, tick: SseTick) {
        let _fanout = self.fanout.lock().unwrap_or_else(|p| p.into_inner());
        let active = {
            let mut subscribers = self.subscribers.lock().unwrap_or_else(|p| p.into_inner());
            let mut active = Vec::with_capacity(subscribers.len());
            subscribers.retain(|_, weak| {
                let Some(slot) = weak.upgrade() else {
                    return false;
                };
                active.push(slot);
                true
            });
            active
        };
        // Do not hold the registry while publishing or releasing the final Arc:
        // a concurrently disconnected slot unregisters itself in its destructor.
        for slot in active {
            slot.publish(tick);
        }
    }

    #[cfg(test)]
    pub(crate) fn subscriber_count(&self) -> usize {
        self.subscribers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }
}
