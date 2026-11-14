//! Keep raster admission until the transport finishes writing its response.
//!
//! Tool registries are request-local and drop before socket writes. A response
//! scope retains their admission without retaining the scene or renderer. The
//! stack supports nested dispatch and is confined to its connection thread.

use std::{cell::RefCell, marker::PhantomData, rc::Rc, sync::Arc};

use crate::export::RasterAdmission;

thread_local! {
    static RESPONSES: RefCell<Vec<Response>> = const { RefCell::new(Vec::new()) };
}

struct Response {
    id: usize,
    admissions: Vec<Arc<RasterAdmission>>,
}

#[must_use = "hold this scope until response writing and flushing have finished"]
pub(crate) struct RasterResponseScope {
    id: usize,
    _connection_thread: PhantomData<Rc<()>>,
}

impl RasterResponseScope {
    pub(crate) fn new() -> Self {
        let id = RESPONSES.with(|responses| {
            let mut responses = responses.borrow_mut();
            let id = responses.last().map_or(0, |response| response.id + 1);
            responses.push(Response {
                id,
                admissions: Vec::new(),
            });
            id
        });
        Self {
            id,
            _connection_thread: PhantomData,
        }
    }
}

pub(super) fn retain(admission: &Arc<RasterAdmission>) {
    RESPONSES.with(|responses| {
        if let Some(response) = responses.borrow_mut().last_mut() {
            response.admissions.push(Arc::clone(admission));
        }
    });
}

impl Drop for RasterResponseScope {
    fn drop(&mut self) {
        let admissions = RESPONSES.with(|responses| {
            let mut responses = responses.borrow_mut();
            responses
                .iter()
                .position(|response| response.id == self.id)
                .map(|index| responses.remove(index))
        });
        drop(admissions);
    }
}

#[cfg(test)]
#[path = "raster_response_tests.rs"]
mod tests;
