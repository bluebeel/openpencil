use std::sync::atomic::{AtomicU64, Ordering};
pub(super) struct Metrics {
    pub admitted: AtomicU64,
    pub fallback: AtomicU64,
    pub cohorts: AtomicU64,
    pub saves: AtomicU64,
    pub changed: AtomicU64,
    pub isolated: AtomicU64,
    pub rolled_back: AtomicU64,
    pub capture_refusals: AtomicU64,
    pub claimed: [AtomicU64; 256],
    pub saved: [AtomicU64; 256],
}
impl Default for Metrics {
    fn default() -> Self {
        Self {
            admitted: AtomicU64::new(0),
            fallback: AtomicU64::new(0),
            cohorts: AtomicU64::new(0),
            saves: AtomicU64::new(0),
            changed: AtomicU64::new(0),
            isolated: AtomicU64::new(0),
            rolled_back: AtomicU64::new(0),
            capture_refusals: AtomicU64::new(0),
            claimed: std::array::from_fn(|_| AtomicU64::new(0)),
            saved: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}
impl Metrics {
    pub(super) fn snapshot(&self) -> serde_json::Value {
        let histogram = |values: &[AtomicU64]| {
            values
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    (
                        (i + 1).to_string(),
                        serde_json::Value::from(n.load(Ordering::Relaxed)),
                    )
                })
                .collect::<serde_json::Map<_, _>>()
        };
        serde_json::json!({"admitted":self.admitted.load(Ordering::Relaxed),
            "capacityFallback":self.fallback.load(Ordering::Relaxed),"cohorts":self.cohorts.load(Ordering::Relaxed),
            "durableSaves":self.saves.load(Ordering::Relaxed),"committedChanges":self.changed.load(Ordering::Relaxed),
            "isolated":self.isolated.load(Ordering::Relaxed),"rolledBackRequests":self.rolled_back.load(Ordering::Relaxed),
            "captureBudgetRefusals":self.capture_refusals.load(Ordering::Relaxed),
            "claimedBatchSizes":histogram(&self.claimed),"savedBatchSizes":histogram(&self.saved)})
    }
}
