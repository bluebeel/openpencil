//! Coordinate memory-heavy headless work before its input graph is derived.
//!
//! Raster requests share the existing two-request bound. Figma conversion is
//! exclusive with those requests, including their retained encoded responses.
//! This is admission, not an allocator or a process RSS accounting claim.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, OnceLock,
};

pub(crate) const MAX_RENDER_ADMISSIONS: usize = 2;
const CONVERSION: usize = MAX_RENDER_ADMISSIONS + 1;

/// Cloneable gate for one service's expensive render/conversion work.
#[derive(Clone, Default)]
pub struct SharedWorkGate {
    state: Arc<AtomicUsize>,
}

/// Capacity is released on normal return, disconnect, or unwind.
#[must_use = "hold this lease through processing and response writing"]
pub struct SharedWorkLease {
    gate: SharedWorkGate,
    units: usize,
}

impl SharedWorkGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve one render request, before layout derivation or scene cloning.
    pub fn try_render(&self) -> Option<SharedWorkLease> {
        self.state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                (state < MAX_RENDER_ADMISSIONS).then_some(state + 1)
            })
            .ok()
            .map(|_| SharedWorkLease {
                gate: self.clone(),
                units: 1,
            })
    }

    /// Conversion cannot overlap rendering or another conversion.
    pub fn try_conversion(&self) -> Option<SharedWorkLease> {
        self.state
            .compare_exchange(0, CONVERSION, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| SharedWorkLease {
                gate: self.clone(),
                units: CONVERSION,
            })
    }
}

impl Drop for SharedWorkLease {
    fn drop(&mut self) {
        self.gate.state.fetch_sub(self.units, Ordering::AcqRel);
    }
}

/// Production raster and conversion routes use this same process-local gate.
pub fn shared_work_gate() -> &'static SharedWorkGate {
    static GATE: OnceLock<SharedWorkGate> = OnceLock::new();
    GATE.get_or_init(SharedWorkGate::new)
}

#[cfg(test)]
#[path = "shared_work_tests.rs"]
mod tests;
