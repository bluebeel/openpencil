//! A fence belongs to one locked document authority, not a connection or agent.
//! An incomplete operation may have renamed its file. Never replay it or expose
//! its memory; only installing a verified authoritative load clears this fence.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

pub(crate) const REQUIRED: &str = "document-recovery-required: an earlier write outcome is unconfirmed; explicitly reload the authoritative saved file before accessing this document; do not replay the failed request";

#[derive(Clone, Default)]
pub(crate) struct DocumentRecovery(Arc<AtomicU64>);

impl DocumentRecovery {
    pub(crate) fn epoch(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
    pub(crate) fn required(&self) -> bool {
        self.epoch() & 1 != 0
    }
    pub(crate) fn trip(&self) {
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                (epoch & 1 == 0).then(|| epoch.saturating_add(1))
            });
    }
    pub(crate) fn trip_at(&self, epoch: u64) {
        if epoch & 1 == 0 {
            let _ = self.0.compare_exchange(
                epoch,
                epoch.saturating_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
    pub(crate) fn replaced(&self) {
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                if epoch & 1 != 0 {
                    None
                } else {
                    Some(epoch.checked_add(2).unwrap_or(u64::MAX))
                }
            });
    }
    /// Caller must hold the authority lock and already have installed disk.
    /// Exhaustion stays fenced rather than allowing an epoch ABA.
    pub(crate) fn recovered(&self, expected: u64) -> bool {
        expected & 1 != 0
            && expected.checked_add(1).is_some_and(|next| {
                self.0
                    .compare_exchange(expected, next, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            })
    }
    pub(crate) fn watch(&self) -> IncompleteOperation {
        IncompleteOperation {
            fence: self.clone(),
            completed: false,
        }
    }
}

/// Also trips when an inner actor-context scope catches the unwind, so mutex
/// poisoning is neither necessary nor sufficient to detect an uncertain edit.
pub(crate) struct IncompleteOperation {
    fence: DocumentRecovery,
    completed: bool,
}
impl IncompleteOperation {
    pub(crate) fn complete(mut self) {
        self.completed = true;
    }
}
impl Drop for IncompleteOperation {
    fn drop(&mut self) {
        if !self.completed {
            self.fence.trip();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caught_inner_unwind_trips_before_authority_can_be_observed() {
        let fence = DocumentRecovery::default();
        let observer = fence.clone();
        let _ = std::panic::catch_unwind(|| {
            let _operation = fence.watch();
            panic!("post-rename");
        });
        assert!(observer.required());
        assert_eq!(observer.epoch(), 1);
    }
    #[test]
    fn verified_reload_advances_epoch_and_stale_recovery_cannot_clear_new_failure() {
        let fence = DocumentRecovery::default();
        fence.trip();
        let old = fence.epoch();
        assert!(fence.recovered(old));
        assert_eq!(fence.epoch(), 2);
        fence.trip();
        assert_eq!(fence.epoch(), 3);
        assert!(!fence.recovered(old));
        assert!(fence.required());
    }
    #[test]
    fn successful_and_known_rejected_operations_do_not_fence_other_documents() {
        let a = DocumentRecovery::default();
        let b = DocumentRecovery::default();
        a.watch().complete();
        assert!(!a.required());
        drop(a.watch());
        assert!(a.required());
        assert!(!b.required());
    }
    #[test]
    fn exhausted_epoch_cannot_recover_or_wrap() {
        let fence = DocumentRecovery(Arc::new(AtomicU64::new(u64::MAX)));
        fence.trip();
        assert!(!fence.recovered(u64::MAX));
        assert!(fence.required());
    }

    #[test]
    fn stale_queue_failure_cannot_refence_a_new_loaded_authority() {
        let fence = DocumentRecovery::default();
        let queued = fence.epoch();
        fence.trip_at(queued);
        assert!(fence.recovered(fence.epoch()));
        fence.trip_at(queued);
        assert!(!fence.required());
    }

    #[test]
    fn healthy_replacement_invalidates_old_queue_epoch_without_fencing() {
        let fence = DocumentRecovery::default();
        let queued = fence.epoch();
        fence.replaced();
        assert_ne!(fence.epoch(), queued);
        assert!(!fence.required());
        fence.trip_at(queued);
        assert!(!fence.required());
        assert!(REQUIRED.contains("do not replay"));
    }
}
