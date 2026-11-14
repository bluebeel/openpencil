//! One document authority per account.
//!
//! The local daemon has exactly one `Mutex<WebCanvasState>` and one
//! [`SseHub`]; the online daemon has one of each **per tenant**, and a
//! request reaches the pair belonging to whoever was verified for that
//! connection. Everything that used to be process-wide state and is still
//! process-wide is refused by [`ServeMode`](super::online_policy::ServeMode)
//! instead of being shared.
//!
//! ## Why a lease and not a timestamp
//!
//! Eviction has to reclaim idle tenants without ever pulling one out from
//! under a live connection: a request that is mid-flight holds a `&Mutex<..>`
//! into the tenant, and an SSE stream holds it for minutes. So a connection
//! takes a [`TenantLease`] while executing (and throughout SSE), and eviction only removes
//! tenants whose lease count is zero. Both the lease increment and the
//! eviction check happen under the registry lock, so there is no window where
//! a lease is being taken while its tenant is being removed.
//! Verified requests waiting in the fair scheduler have no borrowed document
//! or lease; their owner/ACL is resolved again when a worker executes them.
//!
//! ## Why removal is a compare-and-remove
//!
//! M4 will persist a tenant's document as it is evicted. If eviction removed
//! the map entry first and wrote afterwards, a returning request could create
//! a *second*, empty tenant for the same account while the first is still
//! being written — two live authorities for one document, and whichever
//! finished last wins. [`TenantRegistry::evict_idle`] therefore resolves the
//! victim, and removes it only after re-checking that the entry in the map is
//! still the exact `Arc` it decided to evict.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use op_editor_core::EditorState;

use super::tenant_auth::ResolvedIdentity;
use super::tenant_store::{TenantStore, TenantStoreError};

/// One edit to a tenant's access list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AclChange {
    Grant(String),
    Revoke(String),
}

/// The result of an applied access-list change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclUpdate {
    /// Whether the list actually moved (a repeated grant does not).
    pub changed: bool,
    /// The list as it now stands, both in memory and on disk.
    pub shared_with: BTreeSet<String>,
}
use super::{SseHub, WebCanvasState};

/// Global connection ceiling for the online daemon.
pub const MAX_CONNS_ENV: &str = "OPENPENCIL_ONLINE_MAX_CONNS";
/// Per-tenant connection ceiling — one account cannot starve the rest.
pub const MAX_CONNS_PER_TENANT_ENV: &str = "OPENPENCIL_ONLINE_MAX_CONNS_PER_TENANT";
/// How many accounts may hold a live document at once.
pub const MAX_TENANTS_ENV: &str = "OPENPENCIL_ONLINE_MAX_TENANTS";
/// Idle seconds after which a tenant with no connections is reclaimed.
pub const IDLE_EVICT_SECS_ENV: &str = "OPENPENCIL_ONLINE_IDLE_EVICT_SECS";

pub const DEFAULT_MAX_CONNS: usize = 4096;
// One thousand subscribers plus one thousand queued/request calls fit without
// creating one request worker or document copy per connection.
pub const DEFAULT_MAX_CONNS_PER_TENANT: usize = 2048;
pub const DEFAULT_MAX_TENANTS: usize = 100;
pub const DEFAULT_IDLE_EVICT_SECS: u64 = 1800;

/// Resource ceilings for one online daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantLimits {
    pub max_conns: usize,
    pub max_conns_per_tenant: usize,
    pub max_tenants: usize,
    pub idle_evict_secs: u64,
}

impl Default for TenantLimits {
    fn default() -> Self {
        Self {
            max_conns: DEFAULT_MAX_CONNS,
            max_conns_per_tenant: DEFAULT_MAX_CONNS_PER_TENANT,
            max_tenants: DEFAULT_MAX_TENANTS,
            idle_evict_secs: DEFAULT_IDLE_EVICT_SECS,
        }
    }
}

impl TenantLimits {
    /// Read the ceilings from the environment, falling back to the defaults.
    ///
    /// An unparseable or zero value keeps the default rather than disabling
    /// the ceiling: a typo in a deployment variable must not silently remove
    /// a resource bound.
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            max_conns: env_usize(MAX_CONNS_ENV, defaults.max_conns),
            max_conns_per_tenant: env_usize(
                MAX_CONNS_PER_TENANT_ENV,
                defaults.max_conns_per_tenant,
            ),
            max_tenants: env_usize(MAX_TENANTS_ENV, defaults.max_tenants),
            idle_evict_secs: env_u64(IDLE_EVICT_SECS_ENV, defaults.idle_evict_secs),
        }
    }
}

fn env_usize(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn env_u64(name: &str, fallback: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

/// Why the registry could not hand out a tenant.
///
/// Both variants are capacity verdicts, never a statement about the
/// requesting account — a caller learns that the service is full, not
/// anything about other tenants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantError {
    /// The daemon already holds [`TenantLimits::max_tenants`] documents and
    /// none of them is idle enough to reclaim.
    TooManyTenants,
    /// This account already holds [`TenantLimits::max_conns_per_tenant`]
    /// connections.
    TooManyConnections,
    /// The caller asked for another account's tenant and is not on its
    /// access list.
    NotShared,
}

impl TenantError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::TooManyTenants => "server-busy",
            Self::TooManyConnections => "too-many-connections",
            Self::NotShared => "tenant-not-shared",
        }
    }

    pub const fn http_status(self) -> &'static str {
        match self {
            Self::TooManyTenants | Self::TooManyConnections => "503 Service Unavailable",
            // A forbidden share and a non-existent one answer the same way:
            // otherwise the difference is an oracle for which accounts exist.
            Self::NotShared => "403 Forbidden",
        }
    }
}

impl std::fmt::Display for TenantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyTenants => f.write_str("server busy"),
            Self::TooManyConnections => {
                f.write_str("too many concurrent connections for this account")
            }
            Self::NotShared => f.write_str("this document is not shared with you"),
        }
    }
}

impl std::error::Error for TenantError {}

/// One account's live document authority.
pub struct Tenant {
    pub(super) authority: Arc<super::ConnAuthority>,
    /// This account's editor, version counter and collaboration state — the
    /// exact structure the single-user daemon keeps, one per account.
    pub(crate) state: Mutex<WebCanvasState>,
    /// This account's SSE subscribers. Separate per tenant so a version bump
    /// is only ever broadcast to the account that caused it.
    pub(crate) hub: SseHub,
    /// Accounts this tenant's owner has shared the document with.
    ///
    /// A `BTreeSet` rather than a `HashSet` so the persisted list has a
    /// stable order and two saves of the same ACL produce the same bytes.
    /// Separate from the state mutex because admission is checked on every
    /// request while the document lock may be held by a long push.
    shared_with: Mutex<BTreeSet<String>>,
    /// Live leases. Non-zero means "do not evict".
    leases: AtomicUsize,
    maintenance: AtomicUsize,
    /// Unix seconds of the last lease acquire or release.
    last_active_unix: AtomicU64,
}

impl Tenant {
    fn new(
        port: u16,
        allow_origins: &[String],
        editor: EditorState,
        shared_with: BTreeSet<String>,
        now_unix: u64,
    ) -> Self {
        let mut state = WebCanvasState::new_for_tenant(editor, port);
        // Every tenant answers for the same public origin; the allowlist is a
        // deployment property, not an account one.
        state.allow_origins = allow_origins.to_vec();
        Self {
            authority: Arc::new(super::ConnAuthority::new(&state)),
            state: Mutex::new(state),
            hub: SseHub::default(),
            shared_with: Mutex::new(shared_with),
            leases: AtomicUsize::new(0),
            maintenance: AtomicUsize::new(0),
            last_active_unix: AtomicU64::new(now_unix),
        }
    }

    /// Whether `visitor` may reach this tenant's document.
    pub fn admits(&self, visitor: &str) -> bool {
        self.shared_with
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(visitor)
    }

    /// Add an account to the access list. Returns whether it was new.
    pub fn grant(&self, visitor: &str) -> bool {
        self.shared_with
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(visitor.to_string())
    }

    /// Remove an account. Returns whether it had been granted.
    pub fn revoke(&self, visitor: &str) -> bool {
        self.shared_with
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(visitor)
    }

    /// The access list, locked for a read-modify-write.
    ///
    /// `update_acl` holds this across both the edit and the disk write so two
    /// concurrent grants cannot each write back a list missing the other's.
    pub(super) fn shared_with_guard(&self) -> std::sync::MutexGuard<'_, BTreeSet<String>> {
        self.shared_with.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A snapshot of the access list.
    pub fn shared_with(&self) -> BTreeSet<String> {
        self.shared_with
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Live lease count. Zero is the only evictable value.
    pub fn lease_count(&self) -> usize {
        self.leases.load(Ordering::Acquire)
    }

    fn touch(&self, now_unix: u64) {
        self.last_active_unix.store(now_unix, Ordering::Release);
    }

    fn idle_secs(&self, now_unix: u64) -> u64 {
        now_unix.saturating_sub(self.last_active_unix.load(Ordering::Acquire))
    }
}

/// A connection's claim on a tenant.
///
/// Held for the whole request (including a long-lived SSE stream). While one
/// exists the tenant cannot be evicted, so the borrows a connection takes out
/// of it stay valid.
pub struct TenantLease {
    tenant: Arc<Tenant>,
    /// The account this lease resolved to — the OWNER of the document, which
    /// for a shared visit is not the visitor.
    user_id: String,
}

impl std::fmt::Debug for TenantLease {
    /// Deliberately opaque: a lease points at an account's whole editor, and
    /// a derived `Debug` would print its document and its credentials into
    /// whatever log or test failure rendered it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantLease")
            .field("leases", &self.tenant.lease_count())
            .finish_non_exhaustive()
    }
}

impl TenantLease {
    /// The owning account id. For a shared visit this is the owner, not the
    /// visitor — it is the tenant's key, not the caller's identity.
    pub fn owner_id(&self) -> &str {
        &self.user_id
    }

    pub fn tenant(&self) -> &Tenant {
        &self.tenant
    }

    pub fn state(&self) -> &Mutex<WebCanvasState> {
        &self.tenant.state
    }

    pub fn hub(&self) -> &SseHub {
        &self.tenant.hub
    }
}

impl Drop for TenantLease {
    fn drop(&mut self) {
        // Publish the final-use timestamp BEFORE making the tenant evictable.
        // An acquire load observing zero also observes this last release time.
        // A just-finished long SSE stream must not look idle in between them.
        self.tenant.touch(now_unix());
        self.tenant.leases.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Counts requests that are inside a document write, and refuses new ones
/// once shutdown has begun.
///
/// Draining connections alone is not enough: a worker can be past the drain
/// check and about to take the state lock when the flush snapshots the
/// document. It then commits and answers 200 for work the flush never saw.
/// The barrier closes that window — shutdown stops admitting writes and then
/// waits for the ones already inside to finish.
#[derive(Debug, Default)]
pub struct WriteBarrier {
    active: AtomicUsize,
    closed: AtomicBool,
}

impl WriteBarrier {
    /// Enter the write path, unless shutdown has closed it.
    ///
    /// `None` means the caller must refuse the request (503): the daemon is
    /// stopping and cannot durably accept a write.
    pub fn enter(&self) -> Option<WritePass<'_>> {
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        self.active.fetch_add(1, Ordering::AcqRel);
        // Re-checked after the increment: a close landing between the two
        // would otherwise admit a writer the drain has already stopped
        // waiting for.
        if self.closed.load(Ordering::Acquire) {
            self.active.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(WritePass { barrier: self })
    }

    /// Enter the write path regardless of the closed flag. Tests only — it is
    /// how a test stands in for a writer that was admitted before the close.
    #[cfg(test)]
    pub fn enter_for_test(&self) -> WritePass<'_> {
        self.active.fetch_add(1, Ordering::AcqRel);
        WritePass { barrier: self }
    }

    /// Stop admitting writes. Idempotent.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
}

/// Proof that a write is in flight. Decrements on drop.
pub struct WritePass<'a> {
    barrier: &'a WriteBarrier,
}

impl Drop for WritePass<'_> {
    fn drop(&mut self) {
        self.barrier.active.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Every live tenant, keyed by verified account id.
pub struct TenantRegistry {
    tenants: Mutex<HashMap<String, Arc<Tenant>>>,
    transitions: Mutex<HashMap<String, Arc<store_coord::Transition>>>,
    // Preserve the old one-cold-document memory bound, without holding the
    // registry mutex while decoding or touching any document/ACL filesystem.
    cold_load: Mutex<()>,
    limits: TenantLimits,
    /// Reported by `GET /api/mcp/server`; the same bound port for everyone.
    port: u16,
    /// Public origins this deployment answers for, stamped onto every tenant.
    allow_origins: Vec<String>,
    /// Where an evicted tenant is written and a returning one is read from.
    store: TenantStore,
}

impl TenantRegistry {
    pub fn new(port: u16, limits: TenantLimits, allow_origins: Vec<String>) -> Self {
        Self::with_store(port, limits, allow_origins, TenantStore::from_env())
    }

    pub fn with_store(
        port: u16,
        limits: TenantLimits,
        allow_origins: Vec<String>,
        store: TenantStore,
    ) -> Self {
        Self {
            tenants: Mutex::new(HashMap::new()),
            transitions: Mutex::new(HashMap::new()),
            cold_load: Mutex::new(()),
            limits,
            port,
            allow_origins,
            store,
        }
    }

    pub const fn store(&self) -> &TenantStore {
        &self.store
    }

    /// The deployment's public origin allowlist.
    pub fn allow_origins(&self) -> &[String] {
        &self.allow_origins
    }

    pub const fn limits(&self) -> TenantLimits {
        self.limits
    }

    pub fn tenant_count(&self) -> usize {
        self.lock().len()
    }

    /// Take a lease on `identity`'s tenant, creating it if this is the
    /// account's first live connection.
    ///
    /// The key is `identity.user_id` and nothing else — see the module docs
    /// on [`super::tenant_auth`] for why no request-supplied value may ever
    /// reach this argument.
    ///
    /// A tenant that is not in memory is restored from disk when the store
    /// holds one, and starts from [`EditorState::starter`] otherwise — so an
    /// eviction is invisible to the account beyond the first request's cost.
    pub fn lease_for(&self, identity: &ResolvedIdentity) -> Result<TenantLease, TenantError> {
        self.lease_tenant(&identity.user_id)
    }

    /// Verified pending calls need no document materialization/eviction lease.
    /// This grants only a scheduling key; actual dispatch must acquire a fresh
    /// lease and recheck the live share list so queued calls cannot bypass a revoke.
    pub(super) fn queued_owner(
        &self,
        identity: &ResolvedIdentity,
        requested_owner: Option<&str>,
    ) -> Result<String, TenantError> {
        let owner = requested_owner.unwrap_or(&identity.user_id);
        if owner != identity.user_id && !self.admits_visitor(owner, &identity.user_id) {
            return Err(TenantError::NotShared);
        }
        Ok(owner.to_owned())
    }

    /// Take a lease on `owner_id`'s tenant on behalf of `visitor`.
    ///
    /// The owner always passes. Anyone else must appear in the owner's access
    /// list — and note that the list is consulted on EVERY request, so a
    /// revoke takes effect on the visitor's next call rather than whenever
    /// some session expires.
    ///
    /// A tenant that is not resident is restored first: a visitor must be
    /// able to open a shared document whose owner is offline, and refusing
    /// until the owner next signs in would make sharing useless.
    ///
    /// ## Admission precedes materialisation
    ///
    /// Materialising first would let an unauthenticated-in-practice caller
    /// spend the daemon's whole tenant budget: `?tenant=` names an arbitrary
    /// account, and creating the tenant to discover the caller is not on its
    /// list means every refused request still costs a resident tenant. So a
    /// non-resident owner is admitted from the PERSISTED access list, which
    /// reads one small file and materialises nothing.
    ///
    /// A resident owner is checked against the live list, which is
    /// authoritative — a revoke that has not been written yet still takes
    /// effect immediately.
    pub fn lease_for_shared(
        &self,
        owner_id: &str,
        visitor: &ResolvedIdentity,
    ) -> Result<TenantLease, TenantError> {
        if owner_id == visitor.user_id {
            return self.lease_tenant(owner_id);
        }
        if !self.admits_visitor(owner_id, &visitor.user_id) {
            return Err(TenantError::NotShared);
        }
        let lease = self.lease_tenant(owner_id)?;
        // Re-checked against the live list now that the tenant is resident: a
        // revoke may have landed between the two, and the in-memory list is
        // the authority.
        if !lease.tenant().admits(&visitor.user_id) {
            return Err(TenantError::NotShared);
        }
        Ok(lease)
    }

    /// Whether `visitor` is on `owner_id`'s access list, WITHOUT materialising
    /// the tenant.
    ///
    /// Resident tenants answer from memory; the rest answer from the persisted
    /// list. A deployment with no store therefore admits nobody to a
    /// non-resident tenant, which is the fail-closed direction — the share was
    /// never durable in the first place.
    fn admits_visitor(&self, owner_id: &str, visitor: &str) -> bool {
        let resident = self.lock().get(owner_id).cloned();
        if let Some(tenant) = resident {
            return tenant.admits(visitor);
        }
        self.store.load_acl(owner_id).contains(visitor)
    }

    /// Who has shared with `visitor`, across every resident tenant.
    ///
    /// Resident only, and deliberately: a full answer would mean reading every
    /// directory in the store on every call. The owners a visitor is actually
    /// working with are resident by definition, and the visitor can always
    /// open a share they were told about directly.
    pub fn shared_with_visitor(&self, visitor: &str) -> Vec<String> {
        let tenants: Vec<_> = self
            .lock()
            .iter()
            .map(|(id, tenant)| (id.clone(), Arc::clone(tenant)))
            .collect();
        let mut owners: Vec<String> = tenants
            .into_iter()
            .filter(|(owner, tenant)| owner.as_str() != visitor && tenant.admits(visitor))
            .map(|(owner, _)| owner)
            .collect();
        owners.sort_unstable();
        owners
    }

    /// Apply one access-list change and persist the result atomically.
    ///
    /// The edit and its write happen under the SAME lock, for two reasons.
    /// Snapshot-then-write let two concurrent grants each read the list before
    /// the other's insert and write back a version missing it — the second
    /// write silently dropping the first grant. And a write that fails has to
    /// be reported: the previous code logged it and answered 200, so a user
    /// was told a share had succeeded that would vanish on the next restart.
    ///
    /// On a write failure the in-memory change is ROLLED BACK, so memory and
    /// disk agree and the caller's retry starts from a known state. The
    /// alternative — keep it in memory and mark it pending — would mean the
    /// share works until the process restarts and then silently stops, which
    /// is the harder failure to diagnose.
    pub fn update_acl(
        &self,
        user_id: &str,
        tenant: &Tenant,
        change: AclChange,
    ) -> Result<AclUpdate, TenantStoreError> {
        let mut list = tenant.shared_with_guard();
        if let AclChange::Grant(account) = &change {
            // The store writes at most `MAX_SHARED_ACCOUNTS`, so accepting a
            // grant past the ceiling would report success for a share that
            // silently vanishes on the next save. Refuse it instead.
            if !list.contains(account.as_str())
                && list.len() >= super::tenant_store::MAX_SHARED_ACCOUNTS
            {
                return Err(TenantStoreError::ShareLimitReached(
                    super::tenant_store::MAX_SHARED_ACCOUNTS,
                ));
            }
        }
        let changed = match &change {
            AclChange::Grant(account) => list.insert(account.clone()),
            AclChange::Revoke(account) => list.remove(account.as_str()),
        };
        if !changed || !self.store.is_enabled() {
            return Ok(AclUpdate {
                changed,
                shared_with: list.clone(),
            });
        }
        match self.store.save_acl_for(user_id, &list) {
            Ok(()) => Ok(AclUpdate {
                changed,
                shared_with: list.clone(),
            }),
            Err(error) => {
                match &change {
                    AclChange::Grant(account) => list.remove(account.as_str()),
                    AclChange::Revoke(account) => list.insert(account.clone()),
                };
                Err(error)
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Tenant>>> {
        self.tenants.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Seconds since the Unix epoch, saturating to 0 before it.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[path = "tenant_store_coord.rs"]
mod store_coord;

#[cfg(test)]
#[path = "tenant_tests.rs"]
mod tests;
