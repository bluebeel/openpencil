//! Outbound queue admission policy for one peer connection.
//!
//! Split out of `connection.rs` at the 800-line reviewability cap. The rule
//! it owns is small but load-bearing: droppable and undroppable traffic share
//! one bounded outbound queue, and only one of them can afford to be refused.

use std::time::Instant;

use op_collab_transport::{ConnectionDriver, EncodedFrameTransfer, QueueError, RuntimeError};

/// Outbound queue slots droppable presence may never claim.
///
/// The transport's outbound queue is `outbound_queue_items` deep (eight by
/// default). Presence is republished at roughly 30 Hz while a peer drags, so
/// a socket that stalls for a fraction of a second is enough for presence
/// alone to occupy every slot — and the commit that ends the drag is then
/// refused with `QueueError::Full`, which is fatal to the connection. Holding
/// slots back for undroppable frames keeps a stalled socket a delay rather
/// than a teardown.
const RESERVED_RELIABLE_QUEUE_SLOTS: usize = 2;

/// Whether a droppable frame may still take an outbound queue slot.
fn lossy_queue_slot_available(driver: &ConnectionDriver) -> bool {
    driver.queued_items()
        < driver
            .outbound_queue_capacity()
            .saturating_sub(RESERVED_RELIABLE_QUEUE_SLOTS)
}

pub(super) fn queue_command_frame(
    driver: &mut ConnectionDriver,
    encoded: EncodedFrameTransfer,
    coalesce_key: Option<u64>,
    lossy_presence: bool,
    now: Instant,
) -> Result<(), RuntimeError> {
    // Some(key) may be the first update for a different actor. Only a key
    // actually queued can replace an item without claiming a reserved slot.
    if lossy_presence
        && !lossy_queue_slot_available(driver)
        && coalesce_key.is_none_or(|key| !driver.has_queued_coalescing_key(key))
    {
        return Ok(());
    }
    let result = match coalesce_key {
        Some(key) => driver.queue_coalescing_encoded_frame(key, encoded, now),
        None => driver.queue_encoded_frame(encoded, now),
    };
    match result {
        Err(RuntimeError::Queue(QueueError::Full | QueueError::ByteBudget)) if lossy_presence => {
            Ok(())
        }
        result => result,
    }
}

#[cfg(test)]
#[path = "connection_queue_tests.rs"]
mod queue_tests;
