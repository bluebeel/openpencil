//! Cheap production counters make grouping participation directly observable.
use super::{budget, limits};
use std::sync::atomic::{AtomicU64, Ordering};

pub(in crate::web_canvas_server) struct Metrics {
    pub admitted: AtomicU64,
    pub capacity_fallback: AtomicU64,
    pub cohorts: AtomicU64,
    pub applied: AtomicU64,
    pub committed_changes: AtomicU64,
    pub claimed_sizes: [AtomicU64; limits::COHORT_CALLS],
    pub saved_sizes: [AtomicU64; limits::COHORT_CALLS],
    pub saves: AtomicU64,
    pub isolated: AtomicU64,
    pub strict_capture_fallback: AtomicU64,
    refusals: [AtomicU64; 11],
    component_count: AtomicU64,
    component_bytes: AtomicU64,
    target_bytes: AtomicU64,
    auxiliary_bytes: AtomicU64,
    retained_bytes: AtomicU64,
    projected_bytes: AtomicU64,
    last_size_error: AtomicU64,
}
impl Default for Metrics {
    fn default() -> Self {
        Self {
            admitted: AtomicU64::new(0),
            capacity_fallback: AtomicU64::new(0),
            cohorts: AtomicU64::new(0),
            applied: AtomicU64::new(0),
            committed_changes: AtomicU64::new(0),
            saves: AtomicU64::new(0),
            isolated: AtomicU64::new(0),
            strict_capture_fallback: AtomicU64::new(0),
            component_count: AtomicU64::new(0),
            component_bytes: AtomicU64::new(0),
            target_bytes: AtomicU64::new(0),
            auxiliary_bytes: AtomicU64::new(0),
            retained_bytes: AtomicU64::new(0),
            projected_bytes: AtomicU64::new(0),
            last_size_error: AtomicU64::new(0),
            claimed_sizes: std::array::from_fn(|_| AtomicU64::new(0)),
            saved_sizes: std::array::from_fn(|_| AtomicU64::new(0)),
            refusals: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}
impl Metrics {
    pub(super) fn observe_forecast(&self, forecast: budget::Forecast) {
        for (counter, value) in [
            (&self.component_count, forecast.component_count),
            (&self.component_bytes, forecast.components),
            (&self.target_bytes, forecast.target),
            (&self.auxiliary_bytes, forecast.auxiliary),
            (&self.retained_bytes, forecast.retained),
            (&self.projected_bytes, forecast.projected),
        ] {
            counter.fetch_max(value as u64, Ordering::Relaxed);
        }
        self.last_size_error
            .store(forecast.error as u64, Ordering::Relaxed);
    }
    pub(super) fn refuse(&self, reason: budget::Refusal) {
        self.refusals[reason as usize].fetch_add(1, Ordering::Relaxed);
    }
    pub(in crate::web_canvas_server) fn snapshot(&self) -> serde_json::Value {
        self.snapshot_with_histograms(false)
    }
    /// Monitoring under writer saturation needs only occupied histogram bins.
    pub(in crate::web_canvas_server) fn compact_snapshot(&self) -> serde_json::Value {
        self.snapshot_with_histograms(true)
    }
    fn snapshot_with_histograms(&self, sparse: bool) -> serde_json::Value {
        let mut refusals = serde_json::Map::new();
        for reason in budget::Refusal::ALL {
            refusals.insert(
                reason.code().into(),
                self.refusals[reason as usize]
                    .load(Ordering::Relaxed)
                    .into(),
            );
        }
        let histogram = |values: &[AtomicU64]| {
            values
                .iter()
                .enumerate()
                .filter_map(|(i, count)| {
                    let count = count.load(Ordering::Relaxed);
                    (!sparse || count != 0)
                        .then(|| ((i + 1).to_string(), serde_json::Value::from(count)))
                })
                .collect::<serde_json::Map<_, _>>()
        };
        serde_json::json!({
            "admitted":self.admitted.load(Ordering::Relaxed),
            "capacityFallback":self.capacity_fallback.load(Ordering::Relaxed),
            "cohorts":self.cohorts.load(Ordering::Relaxed),
            "appliedBeforeSave":self.applied.load(Ordering::Relaxed),
            "committedChanges":self.committed_changes.load(Ordering::Relaxed),
            "claimedBatchSizes":histogram(&self.claimed_sizes),
            "savedBatchSizes":histogram(&self.saved_sizes),
            "durableSaves":self.saves.load(Ordering::Relaxed),
            "isolated":self.isolated.load(Ordering::Relaxed),
            "strictCaptureFallback":self.strict_capture_fallback.load(Ordering::Relaxed),
            "budgetRefusals":refusals,
            "admissionForecastPeaks":{
                "componentCount":self.component_count.load(Ordering::Relaxed),
                "componentGraphBytes":self.component_bytes.load(Ordering::Relaxed),
                "targetBytes":self.target_bytes.load(Ordering::Relaxed),
                "auxiliaryBytes":self.auxiliary_bytes.load(Ordering::Relaxed),
                "retainedHistoryBytes":self.retained_bytes.load(Ordering::Relaxed),
                "projectedCohortPeakBytes":self.projected_bytes.load(Ordering::Relaxed)
            },
            "lastSizingError":match self.last_size_error.load(Ordering::Relaxed) {1=>"capacity",2=>"serialization",_=>"none"},
            "queueCallsLimit":limits::QUEUED_CALLS,"queueAccountedBytesLimit":limits::QUEUED_BYTES,
            "cohortCallsLimit":limits::COHORT_CALLS,"cohortRequestBytesLimit":limits::COHORT_BYTES,
            "cohortCaptureForecastLimit":op_editor_core::history_snapshot::COMMAND_CAPTURE_ALLOWANCE
        })
    }
}
