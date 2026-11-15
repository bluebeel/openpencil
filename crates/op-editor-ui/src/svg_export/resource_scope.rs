//! Fallible resources discovered by callback-based widget SVG painters.
//! The outer serializer owns the error; no partial document is returned.
use super::SvgExportError;
use std::{cell::RefCell, rc::Rc};
type Slot = Rc<RefCell<Option<SvgExportError>>>;
thread_local! {static SCOPES:RefCell<Vec<Slot>>=const{RefCell::new(Vec::new())};}
pub(super) struct Capture {
    slot: Slot,
}
impl Capture {
    pub fn begin() -> Result<Self, SvgExportError> {
        if SCOPES.with(|s| s.borrow().len() >= 16) {
            return Err(SvgExportError::ResourceNestingLimit { limit: 16 });
        }
        let slot = Rc::new(RefCell::new(None));
        SCOPES.with(|s| s.borrow_mut().push(slot.clone()));
        Ok(Self { slot })
    }
    pub fn finish(self) -> Result<(), SvgExportError> {
        {
            let result = self.slot.borrow_mut().take().map_or(Ok(()), Err);
            result
        }
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        SCOPES.with(|s| {
            let mut s = s.borrow_mut();
            if let Some(i) = s.iter().rposition(|slot| Rc::ptr_eq(slot, &self.slot)) {
                s.remove(i);
            }
        });
    }
}
pub(super) fn record(error: SvgExportError) {
    SCOPES.with(|s| {
        if let Some(slot) = s.borrow().last() {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Some(error);
            }
        }
    });
}
