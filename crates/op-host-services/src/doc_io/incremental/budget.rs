//! Process-wide reservation includes raw bytes, worst-case node index and edits.
use std::sync::atomic::{AtomicUsize, Ordering};
const LIMIT: usize = 512 * 1024 * 1024;
pub(super) const RAW: usize = 256 * 1024 * 1024;
pub(super) const INDEX: usize = 96 * 1024 * 1024;
pub(super) const EDITS: usize = 16 * 1024 * 1024;
static USED: AtomicUsize = AtomicUsize::new(0);
pub(super) struct Reservation(pub usize);
impl Reservation {
    pub fn new() -> Option<Self> {
        let bytes = RAW + INDEX + EDITS;
        USED.fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
            old.checked_add(bytes).filter(|next| *next <= LIMIT)
        })
        .ok()?;
        Some(Self(bytes))
    }
    pub fn shrink(&mut self, bytes: usize) {
        if bytes < self.0 {
            USED.fetch_sub(self.0 - bytes, Ordering::AcqRel);
            self.0 = bytes;
        }
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        USED.fetch_sub(self.0, Ordering::AcqRel);
    }
}
