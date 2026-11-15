//! Per-owner materialization/eviction rendezvous. Global registry guards never
//! cover document decode, state/ACL lock waits, or disk IO for another owner.
use super::*;
use std::sync::Condvar;

pub(super) struct Transition {
    done: Mutex<bool>,
    ready: Condvar,
}
impl Transition {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            done: Mutex::new(false),
            ready: Condvar::new(),
        })
    }
    fn wait(&self) {
        let mut done = self.done.lock().unwrap_or_else(|p| p.into_inner());
        while !*done {
            done = self.ready.wait(done).unwrap_or_else(|p| p.into_inner());
        }
    }
    fn finish(&self) {
        *self.done.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.ready.notify_all();
    }
}
/// Reservation cleanup also runs on decode/serializer unwind. A waiting call
/// re-resolves its own verified owner after notification, never a stale editor.
struct Pending<'a> {
    registry: &'a TenantRegistry,
    id: String,
    marker: Arc<Transition>,
}
impl Drop for Pending<'_> {
    fn drop(&mut self) {
        // Same lock order as admission/eviction; no state or ACL guard remains.
        let _tenants = self.registry.lock();
        let mut transitions = self
            .registry
            .transitions
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if transitions
            .get(&self.id)
            .is_some_and(|current| Arc::ptr_eq(current, &self.marker))
        {
            transitions.remove(&self.id);
        }
        drop(transitions);
        drop(_tenants);
        self.marker.finish();
    }
}
struct Maintenance(Arc<Tenant>);
impl Maintenance {
    /// Must be called under the registry lock before eviction can remove it.
    fn pin(tenant: &Arc<Tenant>) -> Self {
        tenant.maintenance.fetch_add(1, Ordering::AcqRel);
        Self(Arc::clone(tenant))
    }
}
impl Drop for Maintenance {
    fn drop(&mut self) {
        self.0.maintenance.fetch_sub(1, Ordering::AcqRel);
    }
}

enum LeaseStep<'a> {
    Ready(TenantLease),
    Wait(Arc<Transition>),
    Restore(Pending<'a>),
}
impl TenantRegistry {
    pub(super) fn lease_tenant(&self, user_id: &str) -> Result<TenantLease, TenantError> {
        loop {
            let step = {
                let tenants = self.lock();
                let mut transitions = self.transitions.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(marker) = transitions.get(user_id) {
                    LeaseStep::Wait(Arc::clone(marker))
                } else if let Some(tenant) = tenants.get(user_id) {
                    LeaseStep::Ready(self.take_lease(user_id, tenant)?)
                } else {
                    let restoring = transitions
                        .keys()
                        .filter(|id| !tenants.contains_key(*id))
                        .count();
                    if tenants.len().saturating_add(restoring) >= self.limits.max_tenants {
                        return Err(TenantError::TooManyTenants);
                    }
                    let marker = Transition::new();
                    transitions.insert(user_id.to_owned(), Arc::clone(&marker));
                    // Capacity is reserved before the global lock is dropped.
                    // Competing first calls share one decode, not one Doc each.
                    LeaseStep::Restore(Pending {
                        registry: self,
                        id: user_id.to_owned(),
                        marker,
                    })
                }
            };
            match step {
                LeaseStep::Ready(lease) => return Ok(lease),
                LeaseStep::Wait(marker) => marker.wait(),
                LeaseStep::Restore(pending) => {
                    let created = {
                        let _cold = self.cold_load.lock().unwrap_or_else(|p| p.into_inner());
                        Arc::new(Tenant::new(
                            self.port,
                            &self.allow_origins,
                            self.restore_editor(user_id),
                            self.store.load_acl(user_id),
                            now_unix(),
                        ))
                    };
                    let lease = {
                        let mut tenants = self.lock();
                        // The reserved identity is still present until Pending
                        // drops; no second owner can materialize or evict it.
                        assert!(
                            !tenants.contains_key(user_id),
                            "one reserved tenant materialization"
                        );
                        tenants.insert(user_id.to_owned(), Arc::clone(&created));
                        self.take_lease(user_id, &created)?
                    };
                    drop(pending);
                    return Ok(lease);
                }
            }
        }
    }
    fn take_lease(&self, user_id: &str, tenant: &Arc<Tenant>) -> Result<TenantLease, TenantError> {
        if tenant.lease_count() >= self.limits.max_conns_per_tenant {
            return Err(TenantError::TooManyConnections);
        }
        tenant.leases.fetch_add(1, Ordering::AcqRel);
        tenant.touch(now_unix());
        Ok(TenantLease {
            tenant: Arc::clone(tenant),
            user_id: user_id.to_owned(),
        })
    }
    fn restore_editor(&self, user_id: &str) -> EditorState {
        match self.store.load_document(user_id) {
            Ok(state) => state,
            Err(TenantStoreError::Disabled | TenantStoreError::NotStored) => EditorState::starter(),
            Err(error) => {
                eprintln!("openpencil --serve-web --online: starting a fresh document for an account whose stored one could not be loaded ({error})");
                EditorState::starter()
            }
        }
    }
    /// Shutdown still drains the write barrier first. Pin authoritative tenant
    /// identities while saving; only Arc edges are copied, never document data.
    pub fn flush_all(&self) -> usize {
        if !self.store.is_enabled() {
            return 0;
        }
        let tenants: Vec<_> = self
            .lock()
            .iter()
            .map(|(id, tenant)| (id.clone(), Maintenance::pin(tenant)))
            .collect();
        let mut written = 0;
        for (id, pin) in tenants {
            let guard = pin.0.state.lock().unwrap_or_else(|p| p.into_inner());
            let shared = pin.0.shared_with_guard();
            match self.store.save(&id,&guard.editor,&shared) {
                Ok(())=>written+=1,
                Err(error)=>eprintln!("openpencil --serve-web --online: could not flush a tenant on shutdown ({error})"),
            }
            // State/ACL guards release before their maintenance pin.
        }
        written
    }
    /// Only an idle, unleased owner enters an eviction transition. New calls to
    /// that owner wait outside registry/state locks; unrelated owners continue.
    pub fn evict_idle(&self, now: u64) -> usize {
        let candidates: Vec<_> = self
            .lock()
            .iter()
            .filter(|(_, tenant)| {
                tenant.lease_count() == 0
                    && tenant.maintenance.load(Ordering::Acquire) == 0
                    && tenant.idle_secs(now) >= self.limits.idle_evict_secs
            })
            .map(|(id, tenant)| (id.clone(), Arc::clone(tenant)))
            .collect();
        let mut evicted = 0;
        for (id, victim) in candidates {
            let reservation = {
                let tenants = self.lock();
                let mut transitions = self.transitions.lock().unwrap_or_else(|p| p.into_inner());
                let same = tenants
                    .get(&id)
                    .is_some_and(|current| Arc::ptr_eq(current, &victim));
                if !same
                    || transitions.contains_key(&id)
                    || victim.lease_count() != 0
                    || victim.maintenance.load(Ordering::Acquire) != 0
                    || victim.idle_secs(now) < self.limits.idle_evict_secs
                {
                    continue;
                }
                let marker = Transition::new();
                transitions.insert(id.clone(), Arc::clone(&marker));
                (
                    Pending {
                        registry: self,
                        id: id.clone(),
                        marker,
                    },
                    Maintenance::pin(&victim),
                )
            };
            let saved = if self.store.is_enabled() {
                let guard = victim.state.lock().unwrap_or_else(|p| p.into_inner());
                let shared = victim.shared_with_guard();
                match self.store.save(&id, &guard.editor, &shared) {
                    Ok(()) => true,
                    Err(error) => {
                        eprintln!("openpencil --serve-web --online: keeping a tenant resident because it could not be persisted ({error})");
                        false
                    }
                }
            } else {
                true
            };
            if saved {
                let mut tenants = self.lock();
                let same = tenants
                    .get(&id)
                    .is_some_and(|current| Arc::ptr_eq(current, &victim));
                // A concurrent flush can pin the same identity. Keep it resident
                // until that writer finishes, rather than exposing stale owners.
                if same
                    && victim.lease_count() == 0
                    && victim.maintenance.load(Ordering::Acquire) == 1
                {
                    tenants.remove(&id);
                    evicted += 1;
                }
            }
            // Release the pin first, then notify lease waiters after final IO.
            let (pending, pin) = reservation;
            drop(pin);
            drop(pending);
        }
        evicted
    }
}
