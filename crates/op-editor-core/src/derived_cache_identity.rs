//! In-memory cache ownership; cloned/replaced editor states cannot share an ABA key.
use std::sync::{Arc, Weak};

#[derive(Debug)]
pub(crate) struct DerivedCacheIdentity(Arc<()>);
impl Default for DerivedCacheIdentity {
    fn default() -> Self {
        Self(Arc::new(()))
    }
}
impl Clone for DerivedCacheIdentity {
    fn clone(&self) -> Self {
        Self::default()
    }
}
impl DerivedCacheIdentity {
    pub(crate) fn downgrade(&self) -> Weak<()> {
        Arc::downgrade(&self.0)
    }
}
