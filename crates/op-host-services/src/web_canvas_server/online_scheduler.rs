//! Fair verified-owner request execution, separate from socket/SSE admission.
use std::{
    collections::{HashMap, VecDeque},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, Condvar, Mutex, Weak},
    thread,
};

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub workers: usize,
    pub readers: usize,
    pub active_per_owner: usize,
    pub connections_per_owner: usize,
    pub queued: usize,
    pub bytes: usize,
    pub bytes_per_owner: usize,
}
impl Limits {
    pub(super) fn from_env(connections: usize, per_owner: usize) -> Self {
        let workers =
            configured("OPENPENCIL_ONLINE_REQUEST_WORKERS", 64, 256).min(connections.max(1));
        let bytes = configured(
            "OPENPENCIL_ONLINE_QUEUE_BYTES",
            192 * 1024 * 1024,
            512 * 1024 * 1024,
        );
        Self {
            workers,
            readers: configured("OPENPENCIL_ONLINE_INGRESS_WORKERS", 16, 64)
                .min(connections.max(1)),
            active_per_owner: configured("OPENPENCIL_ONLINE_ACTIVE_PER_TENANT", 16, 64)
                .min(workers),
            connections_per_owner: per_owner,
            queued: connections,
            bytes,
            bytes_per_owner: configured(
                "OPENPENCIL_ONLINE_QUEUE_BYTES_PER_TENANT",
                160 * 1024 * 1024,
                512 * 1024 * 1024,
            )
            .min(bytes),
        }
    }
}
fn configured(name: &str, default: usize, ceiling: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .filter(|value: &usize| *value > 0)
        .unwrap_or(default)
        .min(ceiling)
}

#[derive(Clone, Copy)]
pub(super) enum Refusal {
    Closed,
    Connections,
    Pending,
    Bytes,
    OwnerBytes,
}
impl Refusal {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::Closed => "server shutting down",
            Self::Connections => "too many accepted connections for this account",
            Self::Pending => "online request queue is full",
            Self::Bytes => "online pending request memory budget is full",
            Self::OwnerBytes => "account pending request memory budget is full",
        }
    }
}
pub(super) type Job = Box<dyn FnOnce(Result<ConnectionPermit, Refusal>) + Send + 'static>;
struct Pending {
    job: Job,
    permit: ConnectionPermit,
    bytes: usize,
}
#[derive(Default)]
struct Owner {
    pending: VecDeque<Pending>,
    bytes: usize,
    active: usize,
    connections: usize,
    scheduled: bool,
}
#[derive(Default)]
struct State {
    owners: HashMap<Arc<str>, Owner>,
    ready: VecDeque<Arc<str>>,
    queued: usize,
    bytes: usize,
    closed: bool,
}
struct Shared {
    state: Mutex<State>,
    ready: Condvar,
    limits: Limits,
}

/// Cheap admission only. It survives an SSE handoff, but holds no document,
/// active worker slot, request body, or strong scheduler/queue ownership.
pub(crate) struct ConnectionPermit {
    shared: Weak<Shared>,
    owner: Arc<str>,
}
impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let mut state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(owner) = state.owners.get_mut(&self.owner) {
            owner.connections -= 1;
            if owner.connections == 0 && owner.active == 0 && owner.pending.is_empty() {
                state.owners.remove(&self.owner);
            }
        }
    }
}
struct Active {
    shared: Arc<Shared>,
    owner: Arc<str>,
}
impl Drop for Active {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(owner) = state.owners.get_mut(&self.owner) {
            owner.active -= 1;
            if owner.connections == 0 && owner.active == 0 && owner.pending.is_empty() {
                state.owners.remove(&self.owner);
            } else {
                schedule(&mut state, &self.owner, self.shared.limits.active_per_owner);
            }
        }
        self.shared.ready.notify_one();
    }
}
fn schedule(state: &mut State, key: &Arc<str>, maximum: usize) {
    let Some(owner) = state.owners.get_mut(key) else {
        return;
    };
    if !owner.scheduled && !owner.pending.is_empty() && (state.closed || owner.active < maximum) {
        owner.scheduled = true;
        state.ready.push_back(Arc::clone(key));
    }
}

pub(super) struct Queue {
    shared: Arc<Shared>,
    _workers: Vec<thread::JoinHandle<()>>,
}
impl Queue {
    pub(super) fn new(limits: Limits) -> std::io::Result<Self> {
        let mut queue = Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                ready: Condvar::new(),
                limits,
            }),
            _workers: Vec::with_capacity(limits.workers),
        };
        // Fixed bounded workers, never a thread per request/subscriber.
        for index in 0..limits.workers {
            let shared = Arc::clone(&queue.shared);
            match thread::Builder::new()
                .name(format!("op-online-request-{index}"))
                .spawn(move || worker(shared))
            {
                Ok(handle) => queue._workers.push(handle),
                Err(error) => {
                    queue.close();
                    return Err(error);
                }
            }
        }
        Ok(queue)
    }

    /// Only verified, share-authorized owner keys reach this method. Rejected
    /// callbacks run without the queue lock and cannot enter document dispatch.
    pub(super) fn submit(&self, owner: &str, bytes: usize, job: Job) {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        let limits = self.shared.limits;
        let current = state.owners.get(owner);
        let refusal = if state.closed {
            Some(Refusal::Closed)
        } else if current.map_or(0, |owner| owner.connections) >= limits.connections_per_owner {
            Some(Refusal::Connections)
        } else if state.queued >= limits.queued {
            Some(Refusal::Pending)
        } else if bytes > limits.bytes.saturating_sub(state.bytes) {
            Some(Refusal::Bytes)
        } else if bytes
            > limits
                .bytes_per_owner
                .saturating_sub(current.map_or(0, |owner| owner.bytes))
        {
            Some(Refusal::OwnerBytes)
        } else {
            None
        };
        if let Some(reason) = refusal {
            drop(state);
            job(Err(reason));
            return;
        }
        let key = state
            .owners
            .get_key_value(owner)
            .map(|(key, _)| Arc::clone(key))
            .unwrap_or_else(|| Arc::<str>::from(owner));
        let pending = Pending {
            job,
            bytes,
            permit: ConnectionPermit {
                shared: Arc::downgrade(&self.shared),
                owner: Arc::clone(&key),
            },
        };
        let owner = state.owners.entry(Arc::clone(&key)).or_default();
        owner.connections += 1;
        owner.bytes += bytes;
        owner.pending.push_back(pending);
        state.queued += 1;
        state.bytes += bytes;
        schedule(&mut state, &key, limits.active_per_owner);
        self.shared.ready.notify_one();
    }

    pub(super) fn close(&self) {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed {
            return;
        }
        state.closed = true;
        // Previously capacity-blocked owners also need explicit503 replies.
        let keys = state.owners.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            schedule(&mut state, &key, self.shared.limits.active_per_owner);
        }
        self.shared.ready.notify_all();
    }
}
impl Drop for Queue {
    fn drop(&mut self) {
        self.close();
    }
}

fn worker(shared: Arc<Shared>) {
    loop {
        let Some((pending, active, closed)) = next(&shared) else {
            return;
        };
        let Pending { job, permit, .. } = pending;
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if closed {
                drop(permit);
                job(Err(Refusal::Closed));
            } else {
                job(Ok(permit));
            }
        }));
        drop(active);
    }
}
fn next(shared: &Arc<Shared>) -> Option<(Pending, Active, bool)> {
    let mut state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
    loop {
        while let Some(key) = state.ready.pop_front() {
            let closed = state.closed;
            let Some(owner) = state.owners.get_mut(&key) else {
                continue;
            };
            owner.scheduled = false;
            if !closed && owner.active >= shared.limits.active_per_owner {
                continue;
            }
            let Some(pending) = owner.pending.pop_front() else {
                continue;
            };
            owner.active += 1;
            owner.bytes -= pending.bytes;
            if owner.pending.is_empty() && owner.pending.capacity() > 64 {
                owner.pending.shrink_to_fit();
            }
            state.queued -= 1;
            state.bytes -= pending.bytes;
            schedule(&mut state, &key, shared.limits.active_per_owner);
            if !state.ready.is_empty() {
                shared.ready.notify_one();
            }
            return Some((
                pending,
                Active {
                    shared: Arc::clone(shared),
                    owner: key,
                },
                closed,
            ));
        }
        if state.closed && state.queued == 0 {
            return None;
        }
        state = shared.ready.wait(state).unwrap_or_else(|p| p.into_inner());
    }
}
