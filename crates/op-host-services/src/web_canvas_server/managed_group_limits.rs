//! Queue bounds are independent of the exact allocation-based capture budget.
//! A running save lets the bounded ingress backlog fill the next large cohort.
use super::queue;
use std::time::Duration;
pub(super) const QUEUED_CALLS: usize = 1024;
pub(super) const QUEUED_BYTES: usize = 512 * 1024 * 1024;
pub(super) const COHORT_CALLS: usize = 256;
pub(super) const COHORT_BYTES: usize = 64 * 1024 * 1024;
pub(super) fn queue_limits() -> queue::Limits {
    queue::Limits {
        calls: QUEUED_CALLS,
        bytes: QUEUED_BYTES,
        group_calls: COHORT_CALLS,
        group_bytes: COHORT_BYTES,
        wait: Duration::from_millis(2),
    }
}
