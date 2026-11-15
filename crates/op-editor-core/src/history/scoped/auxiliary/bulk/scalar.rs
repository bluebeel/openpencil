//! Exact whole-field inverse with a complete current-value witness.
use super::{exact, mem};
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Patch<T> {
    old: Option<T>,
    expected: Option<T>,
}
impl<T: Clone + PartialEq + serde::Serialize> Patch<T> {
    pub(super) fn compact(old: Option<T>, current: &Option<T>) -> Option<Self> {
        (!exact(&old, current)).then(|| Self {
            old,
            expected: current.clone(),
        })
    }
    pub(super) fn validate(&self, current: &Option<T>) -> bool {
        exact(&self.expected, current)
    }
    pub(super) fn exchange(&mut self, current: &mut Option<T>) {
        mem::swap(&mut self.old, current);
        self.expected = current.clone();
    }
    pub(super) fn bytes(&self) -> usize {
        mem::size_of::<Self>()
            + super::super::super::header::retained_bytes(&self.old)
            + super::super::super::header::retained_bytes(&self.expected)
    }
}
