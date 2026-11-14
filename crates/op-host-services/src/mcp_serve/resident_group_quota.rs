//! Process-wide reserves cover all resident queues/cohorts, not each file alone.
use std::sync::atomic::{AtomicUsize, Ordering};
const QUEUED_LIMIT: usize = 512 * 1024 * 1024;
// Capture forecasts include retained document/history ownership. Keep the
// payload queue bounded independently so large files can still commit cohorts
// instead of falling back to one durable save per accepted writer.
const CAPTURE_LIMIT: usize = 2 * 1024 * 1024 * 1024;
static QUEUED: AtomicUsize = AtomicUsize::new(0);
static CAPTURED: AtomicUsize = AtomicUsize::new(0);
pub(super) struct Reservation {
    used: &'static AtomicUsize,
    limit: usize,
    bytes: usize,
}
impl Reservation {
    pub(super) fn queued(bytes: usize) -> Option<Self> {
        let mut value = Self {
            used: &QUEUED,
            limit: QUEUED_LIMIT,
            bytes: 0,
        };
        value.grow_to(bytes).then_some(value)
    }
    pub(super) fn captures() -> Self {
        Self {
            used: &CAPTURED,
            limit: CAPTURE_LIMIT,
            bytes: 0,
        }
    }
    pub(super) fn grow_to(&mut self, bytes: usize) -> bool {
        let extra = bytes.saturating_sub(self.bytes);
        if extra == 0 {
            return true;
        }
        if self
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |used| {
                used.checked_add(extra).filter(|&next| next <= self.limit)
            })
            .is_err()
        {
            return false;
        }
        self.bytes = self.bytes.max(bytes);
        true
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
pub(super) fn snapshot() -> serde_json::Value {
    serde_json::json!({"queuedAccountedBytes":QUEUED.load(Ordering::Relaxed),
        "captureForecastBytes":CAPTURED.load(Ordering::Relaxed),
        "queuedAccountedBytesLimit":QUEUED_LIMIT,"captureForecastBytesLimit":CAPTURE_LIMIT})
}
