//! Exact bound-authority quality results. No document/UI clone, thread spawn,
//! synthetic scene or cache lock held across diagnostics. Public collectors bypass it.
use op_editor_core::EditorState;
use op_editor_ui::layout_scene::LayoutScene;
use op_mcp::ToolOutcome;
use std::{
    collections::VecDeque,
    mem::size_of,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex, OnceLock, Weak,
    },
};
const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 64;
const MAX_PRODUCERS: usize = 8;
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
struct Charge(usize);
impl Drop for Charge {
    fn drop(&mut self) {
        LIVE_BYTES.fetch_sub(self.0, Ordering::AcqRel);
    }
}
pub(super) struct ResultValue {
    outcome: ToolOutcome,
    _charge: Option<Charge>,
}
impl ResultValue {
    pub(super) fn outcome(&self) -> ToolOutcome {
        self.outcome.clone()
    }
    pub(super) fn into_outcome(self) -> ToolOutcome {
        self.outcome
    }
}
#[derive(Clone)]
struct Key {
    owner: Weak<()>,
    generation: u64,
    revision: u64,
    scene: Weak<LayoutScene>,
    active_page: usize,
    raw_scope: (usize, usize),
}
impl Key {
    fn capture(state: &EditorState, scene: &Arc<LayoutScene>) -> Self {
        let roots = state.active_children();
        Self {
            owner: state.derived_cache_owner(),
            generation: state.document_generation(),
            revision: state.document_revision(),
            scene: Arc::downgrade(scene),
            active_page: state.ui.active_page_index,
            raw_scope: (roots.as_ptr() as usize, roots.len()),
        }
    }
    fn matches(&self, other: &Self) -> bool {
        self.owner.ptr_eq(&other.owner)
            && self.generation == other.generation
            && self.revision == other.revision
            && self.scene.ptr_eq(&other.scene)
            && self.active_page == other.active_page
            && self.raw_scope == other.raw_scope
    }
    fn live(&self) -> bool {
        self.owner.strong_count() != 0 && self.scene.strong_count() != 0
    }
}
struct Entry {
    key: Key,
    value: Weak<ResultValue>,
    // Only a charged value can have a cache-owned strong reference. Oversized
    // results are shared only while an actual caller owns them.
    _owned: Option<Arc<ResultValue>>,
}
enum State {
    Building,
    Ready(Weak<ResultValue>),
    Failed,
}
struct Slot {
    key: Key,
    state: Mutex<State>,
    wake: Condvar,
}
impl Slot {
    fn wait(&self) -> Option<Arc<ResultValue>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match &*state {
                State::Building => state = self.wake.wait(state).unwrap_or_else(|e| e.into_inner()),
                State::Ready(value) => return value.upgrade(),
                State::Failed => return None,
            }
        }
    }
    fn publish(&self, value: Option<&Arc<ResultValue>>) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) =
            value.map_or(State::Failed, |value| State::Ready(Arc::downgrade(value)));
        self.wake.notify_all();
    }
}
struct Store {
    entries: VecDeque<Entry>,
    pending: Vec<Weak<Slot>>,
}
struct Cache {
    store: Mutex<Store>,
    capacity: Condvar,
}
fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Cache {
        store: Mutex::new(Store {
            entries: VecDeque::with_capacity(MAX_ENTRIES),
            pending: Vec::with_capacity(MAX_PRODUCERS),
        }),
        capacity: Condvar::new(),
    })
}
impl Store {
    fn purge(&mut self, dead: &mut Vec<Entry>) {
        let mut index = 0;
        while index < self.entries.len() {
            if !self.entries[index].key.live() || self.entries[index].value.strong_count() == 0 {
                dead.push(self.entries.remove(index).unwrap());
            } else {
                index += 1;
            }
        }
        self.pending.retain(|slot| slot.strong_count() != 0);
    }
    fn hit(&mut self, key: &Key) -> Option<Arc<ResultValue>> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.key.matches(key))?;
        let entry = self.entries.remove(index).unwrap();
        let value = entry.value.upgrade();
        self.entries.push_back(entry);
        value
    }
}
struct Producer {
    slot: Arc<Slot>,
    finished: bool,
}
impl Producer {
    fn release(&self) {
        let mut store = cache().store.lock().unwrap_or_else(|e| e.into_inner());
        store
            .pending
            .retain(|slot| !slot.ptr_eq(&Arc::downgrade(&self.slot)) && slot.strong_count() != 0);
        drop(store);
        cache().capacity.notify_one();
    }
    fn publish(&mut self, value: &Arc<ResultValue>) {
        self.release();
        self.slot.publish(Some(value));
        self.finished = true;
    }
}
impl Drop for Producer {
    fn drop(&mut self) {
        if !self.finished {
            self.release();
            self.slot.publish(None);
        }
    }
}
fn reserve(bytes: usize) -> Option<Charge> {
    // Fixed-capacity entry/build metadata is charged outside the JSON pool.
    let metadata = MAX_ENTRIES * size_of::<Entry>()
        + MAX_PRODUCERS * (size_of::<Slot>() + 64)
        + size_of::<Cache>()
        + 1024;
    LIVE_BYTES
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
            live.checked_add(bytes)
                .filter(|sum| *sum <= MAX_BYTES.saturating_sub(metadata))
        })
        .ok()
        .map(|_| Charge(bytes))
}
fn prepared_value(outcome: ToolOutcome) -> Arc<ResultValue> {
    let bytes = match &outcome {
        ToolOutcome::OkJson(json) => json
            .capacity()
            .saturating_add(size_of::<ResultValue>() + 64),
        _ => {
            return Arc::new(ResultValue {
                outcome,
                _charge: None,
            })
        }
    };
    // Preserve complete output on oversize; do not evict every useful report for
    // a value that cannot be retained under this cache's logical byte bound.
    if bytes > MAX_BYTES {
        return Arc::new(ResultValue {
            outcome,
            _charge: None,
        });
    }
    let mut charge = reserve(bytes);
    for _ in 0..MAX_ENTRIES {
        if charge.is_some() {
            break;
        }
        let old = cache()
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .pop_front();
        let Some(old) = old else {
            break;
        };
        drop(old);
        charge = reserve(bytes);
    }
    Arc::new(ResultValue {
        outcome,
        _charge: charge,
    })
}
/// The caller has the real document authority and passes the exact immutable
/// active-query scene built/validated for that state. The scene identity covers
/// all loader/font/theme/transient-ref inputs; owner/revision and raw scope also
/// cover diagnostics' raw metadata, variables, themes, events and navigation.
pub(super) fn result(
    state: &EditorState,
    scene: &Arc<LayoutScene>,
    build: impl FnOnce() -> ToolOutcome,
) -> Arc<ResultValue> {
    let key = Key::capture(state, scene);
    let mut build = Some(build);
    loop {
        let mut dead = Vec::new();
        let mut store = cache().store.lock().unwrap_or_else(|e| e.into_inner());
        store.purge(&mut dead);
        if !dead.is_empty() {
            drop(store);
            drop(dead);
            continue;
        }
        if let Some(value) = store.hit(&key) {
            return value;
        }
        let pending = store
            .pending
            .iter()
            .filter_map(Weak::upgrade)
            .find(|slot| slot.key.matches(&key));
        if let Some(slot) = pending {
            drop(store);
            if let Some(value) = slot.wait() {
                return value;
            }
            // A failed producer never certifies a partial report. Our retained
            // FnOnce is still unused and may become the next exact producer.
            continue;
        }
        if store.pending.len() >= MAX_PRODUCERS {
            // Existing caller threads wait without polling or retaining an
            // owned editor/report. Unrelated keys can use the other producers.
            store = cache()
                .capacity
                .wait(store)
                .unwrap_or_else(|e| e.into_inner());
            drop(store);
            continue;
        }
        let slot = Arc::new(Slot {
            key: key.clone(),
            state: Mutex::new(State::Building),
            wake: Condvar::new(),
        });
        store.pending.push(Arc::downgrade(&slot));
        drop(store);
        let mut producer = Producer {
            slot,
            finished: false,
        };
        let value = prepared_value(build.take().expect("quality diagnostics run once")());
        let entry = Entry {
            key: key.clone(),
            value: Arc::downgrade(&value),
            _owned: value._charge.as_ref().map(|_| Arc::clone(&value)),
        };
        let old = {
            let mut store = cache().store.lock().unwrap_or_else(|e| e.into_inner());
            let old = (store.entries.len() >= MAX_ENTRIES)
                .then(|| store.entries.pop_front())
                .flatten();
            store.entries.push_back(entry);
            old
        };
        drop(old);
        producer.publish(&value);
        return value;
    }
}
