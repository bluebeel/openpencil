//! Document-use scope. Delivery drops it before immutable snapshots, slow
//! readers or SSE take over; those retain only their own resource leases.
use super::primary_files::{Prepared, PrimaryFiles, ReadUse};
use crate::{document_recovery::DocumentRecovery, mcp_serve::file_ownership::ClaimError};
use std::{cell::RefCell, path::Path, rc::Rc, sync::Arc};
struct Frame {
    owner: Arc<PrimaryFiles>,
    read: Option<ReadUse>,
    prepared: Option<Arc<Prepared>>,
    released: bool,
}
thread_local! {static CURRENT:RefCell<Vec<Rc<RefCell<Frame>>>>=const{RefCell::new(Vec::new())};}
pub(super) struct Scope {
    frame: Rc<RefCell<Frame>>,
}
impl Scope {
    pub(super) fn read(
        owner: Arc<PrimaryFiles>,
        recovery: DocumentRecovery,
    ) -> Result<Self, ClaimError> {
        let inherited = CURRENT.with(|current| {
            current.borrow().iter().rev().any(|frame| {
                let frame = frame.borrow();
                Arc::ptr_eq(&frame.owner, &owner)
                    && !frame.released
                    && (frame.read.is_some() || frame.prepared.is_some())
            })
        });
        if inherited {
            return Ok(Self::enter(Frame {
                owner,
                read: None,
                prepared: None,
                released: false,
            }));
        }
        let read = owner.read(recovery)?;
        Ok(Self::enter(Frame {
            owner,
            read: Some(read),
            prepared: None,
            released: false,
        }))
    }
    pub(super) fn bind(owner: Arc<PrimaryFiles>, path: Option<&Path>) -> Result<Self, ClaimError> {
        if CURRENT.with(|current| {
            current.borrow().iter().any(|frame| {
                let frame = frame.borrow();
                Arc::ptr_eq(&frame.owner, &owner) && !frame.released
            })
        }) {
            return Err(ClaimError::Busy);
        }
        let prepared = owner.prepare(path, true)?;
        Ok(Self::enter(Frame {
            owner,
            read: None,
            prepared: Some(prepared),
            released: false,
        }))
    }
    fn enter(frame: Frame) -> Self {
        let frame = Rc::new(RefCell::new(frame));
        CURRENT.with(|current| current.borrow_mut().push(Rc::clone(&frame)));
        Self { frame }
    }
    pub(super) fn release(&self) {
        let (read, prepared) = {
            let mut frame = self.frame.borrow_mut();
            frame.released = true;
            (frame.read.take(), frame.prepared.take())
        };
        drop(read);
        drop(prepared);
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|current| {
            let mut current = current.borrow_mut();
            if let Some(index) = current
                .iter()
                .position(|frame| Rc::ptr_eq(frame, &self.frame))
            {
                current.remove(index);
            }
        });
    }
}
pub(super) fn release(scope: Option<&Scope>) {
    if let Some(scope) = scope {
        scope.release();
    }
}
pub(super) fn prepared(
    owner: &Arc<PrimaryFiles>,
    path: Option<&Path>,
) -> Result<Arc<Prepared>, ClaimError> {
    let frame = CURRENT.with(|current| {
        current
            .borrow()
            .iter()
            .rev()
            .find(|frame| Arc::ptr_eq(&frame.borrow().owner, owner))
            .cloned()
    });
    if let Some(frame) = frame {
        let frame = frame.borrow();
        if frame.released {
            return Err(ClaimError::Stale);
        }
        let prepared = frame
            .prepared
            .as_ref()
            .filter(|prepared| prepared.owner_is(owner) && prepared.matches(path))
            .ok_or(ClaimError::Busy)?;
        return Ok(Arc::clone(prepared));
    }
    // Generic synchronous helpers may already hold mutable state. Refuse a
    // busy destination instead of waiting through an inverted document lock.
    owner.prepare(path, false)
}
