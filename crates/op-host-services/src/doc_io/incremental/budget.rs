//! Process-wide reservation includes file I/O, node index, edits and thumbnail ownership.
use std::sync::atomic::{AtomicUsize, Ordering};
const LIMIT: usize = 512 * 1024 * 1024;
pub(super) const IO: usize = 256 * 1024;
pub(super) const INDEX: usize = 256 * 1024 * 1024;
pub(super) const EDITS: usize = 16 * 1024 * 1024;
const MAX_SOURCE: usize = 512 * 1024 * 1024;
const SOURCE_LIMIT: usize = 2 * 1024 * 1024 * 1024;
static SOURCES: AtomicUsize = AtomicUsize::new(0);
static USED: AtomicUsize = AtomicUsize::new(0);
pub(super) struct Reservation(pub usize, usize);
impl Reservation {
    pub fn new(thumbnails: usize) -> Option<Self> {
        let bytes = IO
            .checked_add(INDEX)?
            .checked_add(EDITS)?
            .checked_add(thumbnails)?;
        USED.fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
            old.checked_add(bytes).filter(|next| *next <= LIMIT)
        })
        .ok()?;
        Some(Self(bytes, 0))
    }
    pub fn reserve_source(&mut self, bytes: usize) -> bool {
        if bytes > MAX_SOURCE || self.1 != 0 {
            return false;
        }
        if SOURCES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                old.checked_add(bytes).filter(|next| *next <= SOURCE_LIMIT)
            })
            .is_err()
        {
            return false;
        }
        self.1 = bytes;
        true
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
        SOURCES.fetch_sub(self.1, Ordering::AcqRel);
    }
}
