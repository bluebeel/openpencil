//! Bounded process counters, never per-agent/node keys or payload logs.
use std::sync::atomic::{AtomicU64, Ordering};
#[derive(Clone, Copy)]
#[repr(usize)]
pub(super) enum Event {
    MutableInvalidation,
    ReceiptRefusal,
    MetadataMismatch,
    RestoreMismatch,
    FullSave,
    IncrementalSave,
    SourceDrift,
    ThumbChange,
    MetaLimit,
    SeedAttempt,
    SeedReady,
    SeedBudget,
    SeedSource,
    SeedIndex,
    SeedCharge,
}
const N: usize = 15;
static COUNTS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
pub(super) fn hit(event: Event) {
    COUNTS[event as usize].fetch_add(1, Ordering::Relaxed);
}
pub(crate) fn snapshot() -> serde_json::Value {
    let names = [
        "mutableInvalidations",
        "receiptRefusals",
        "metadataMismatches",
        "restoreMismatches",
        "fullSaves",
        "incrementalSaves",
        "sourceDrifts",
        "thumbnailChanges",
        "metadataLimits",
        "seedAttempts",
        "seedReady",
        "seedBudgetRefusals",
        "seedSourceRefusals",
        "seedIndexRefusals",
        "seedChargeRefusals",
    ];
    let mut values = serde_json::Map::new();
    for (name, value) in names.iter().zip(&COUNTS) {
        values.insert(
            (*name).into(),
            serde_json::Value::from(value.load(Ordering::Relaxed)),
        );
    }
    values.insert(
        "source".into(),
        "completed canonical staging file; no full encoded RAM copy".into(),
    );
    serde_json::Value::Object(values)
}
