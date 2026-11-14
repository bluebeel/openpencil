//! Mixed geometry/rectangle cache values, including evicted live Arc accounting.
use super::*;

#[path = "page_query_index.rs"]
mod page_query_index;
pub(crate) use page_query_index::PageIndex;

pub(super) type RootRectangles = BTreeMap<String, [f32; 4]>;

#[derive(Clone)]
enum Value {
    Geometry(Arc<Vec<ComputedGeometry>>),
    Rectangles(Arc<RootRectangles>),
    PageIndex(Option<Arc<PageIndex>>),
}
impl Value {
    fn bytes(&self) -> usize {
        64usize.saturating_add(match self {
            Self::Geometry(nodes) => geometry_bytes(nodes),
            Self::PageIndex(index) => index.as_ref().map_or(0, |index| index.storage_bytes()),
            Self::Rectangles(rects) => {
                // A root BTree node can be sparse. Other nodes have at least
                // five entries;128 extra bytes per occupied key conservatively
                // covers spare slots, parent/child pointers and alignment.
                size_of::<RootRectangles>()
                    + if rects.is_empty() { 0 } else { 1024 }
                    + rects.len() * (size_of::<String>() + size_of::<[f32; 4]>() + 128)
                    + rects.keys().map(String::capacity).sum::<usize>()
            }
        })
    }
    fn weak(&self) -> WeakValue {
        match self {
            Self::Geometry(value) => WeakValue::Geometry(Arc::downgrade(value)),
            Self::Rectangles(value) => WeakValue::Rectangles(Arc::downgrade(value)),
            Self::PageIndex(value) => WeakValue::PageIndex(value.as_ref().map(Arc::downgrade)),
        }
    }
}

enum WeakValue {
    Geometry(Weak<Vec<ComputedGeometry>>),
    Rectangles(Weak<RootRectangles>),
    PageIndex(Option<Weak<PageIndex>>),
}
impl WeakValue {
    fn live(&self) -> bool {
        match self {
            Self::Geometry(value) => value.strong_count() != 0,
            Self::Rectangles(value) => value.strong_count() != 0,
            Self::PageIndex(value) => value
                .as_ref()
                .is_some_and(|value| value.strong_count() != 0),
        }
    }
}

pub(super) struct Entry {
    pub(super) key: Key,
    value: Value,
    bytes: usize,
    value_bytes: usize,
}
pub(super) struct Retired {
    value: WeakValue,
    bytes: usize,
}
pub(super) struct Store {
    pub(super) entries: Vec<Entry>,
    pub(super) retired: Vec<Retired>,
    // Both resident entries and externally held evicted values are included.
    pub(super) bytes: usize,
}
impl Store {
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::with_capacity(MAX_ENTRIES),
            retired: Vec::with_capacity(MAX_ENTRIES),
            bytes: 0,
        }
    }
    pub(super) fn base_bytes(&self) -> usize {
        size_of::<Self>()
            + self.entries.capacity() * size_of::<Entry>()
            + self.retired.capacity() * size_of::<Retired>()
    }
    fn purge_retired(&mut self) {
        let mut i = 0;
        while i < self.retired.len() {
            if !self.retired[i].value.live() {
                self.bytes -= self.retired.remove(i).bytes;
            } else {
                i += 1;
            }
        }
    }
    fn retire(&mut self, index: usize) -> Entry {
        let entry = self.entries.remove(index);
        // The complete entry remains alive until dropped outside the lock.
        // Its value stays charged until every strong holder has released it.
        self.bytes -= entry.bytes - entry.value_bytes;
        self.retired.push(Retired {
            value: entry.value.weak(),
            bytes: entry.value_bytes,
        });
        entry
    }
    fn take_dead(&mut self, dead: &mut Vec<Entry>) {
        self.purge_retired();
        let mut i = 0;
        while i < self.entries.len() {
            if self.entries[i].key.owner.strong_count() == 0 {
                dead.push(self.retire(i));
            } else {
                i += 1;
            }
        }
    }
    fn hit(&mut self, key: &Key) -> Option<Value> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.key.matches(key))?;
        let entry = self.entries.remove(index);
        let value = entry.value.clone();
        self.entries.push(entry);
        Some(value)
    }
}

fn get_or_compute_value(
    store: &Mutex<Store>,
    key: Option<Key>,
    compute: impl FnOnce() -> Value,
) -> Value {
    let Some(key) = key else {
        return compute();
    };
    let mut dead = Vec::new();
    let hit = {
        let mut locked = store.lock().unwrap_or_else(|e| e.into_inner());
        locked.take_dead(&mut dead);
        locked.hit(&key)
    };
    drop(dead);
    if let Some(value) = hit {
        return value;
    }
    // Uncached request work does not hold the cache lock. It is not hidden in
    // the shared budget: only admission makes this value cache-retained/shared.
    let value = compute();
    let value_bytes = value.bytes();
    let bytes = key.bytes().saturating_add(value_bytes);
    for _ in 0..=MAX_ENTRIES {
        let evicted = {
            let mut locked = store.lock().unwrap_or_else(|e| e.into_inner());
            locked.purge_retired();
            if let Some(existing) = locked.hit(&key) {
                return existing;
            }
            if bytes.saturating_add(locked.base_bytes()) > MAX_BYTES {
                return value;
            }
            if locked.entries.len() + locked.retired.len() < MAX_ENTRIES
                && locked
                    .bytes
                    .saturating_add(bytes)
                    .saturating_add(locked.base_bytes())
                    <= MAX_BYTES
            {
                locked.bytes += bytes;
                locked.entries.push(Entry {
                    key,
                    value: value.clone(),
                    bytes,
                    value_bytes,
                });
                return value;
            }
            if locked.entries.is_empty() {
                return value;
            }
            locked.retire(0)
        };
        // Free cache-only values before another admission attempt. Any value
        // still held by a caller remains in the retired ledger and cannot be
        // replaced by an uncharged second shared allocation.
        drop(evicted);
    }
    value // Bounded contention fallback; caller-owned uncached request data.
}

pub(super) fn get_or_compute(
    store: &Mutex<Store>,
    key: Option<Key>,
    compute: impl FnOnce() -> Vec<ComputedGeometry>,
) -> Arc<Vec<ComputedGeometry>> {
    match get_or_compute_value(store, key, || Value::Geometry(Arc::new(compute()))) {
        Value::Geometry(value) => value,
        Value::Rectangles(_) | Value::PageIndex(_) => {
            unreachable!("cache key kinds must be separate")
        }
    }
}

pub(super) fn rectangle_key(state: &EditorState, page: usize, parent: &str) -> Option<Key> {
    Key::new(state, page, Some(parent), crate::current_font_generation()).map(|mut key| {
        key.parent_query = None;
        key.root_rectangles = true;
        key
    })
}

pub(super) fn get_rectangles(
    store: &Mutex<Store>,
    key: Option<Key>,
    compute: impl FnOnce() -> RootRectangles,
) -> Arc<RootRectangles> {
    match get_or_compute_value(store, key, || Value::Rectangles(Arc::new(compute()))) {
        Value::Rectangles(value) => value,
        Value::Geometry(_) | Value::PageIndex(_) => {
            unreachable!("cache key kinds must be separate")
        }
    }
}

/// Trusted-host-only rectangle reuse, with the same authoritative root key.
pub(crate) fn root_rectangles_cached(
    state: &EditorState,
    page: usize,
    parent: &str,
    compute: impl FnOnce() -> RootRectangles,
) -> Arc<RootRectangles> {
    get_rectangles(shared_store(), rectangle_key(state, page, parent), compute)
}

/// Opt-in trusted host only. Raw id/location data depends on authority,
/// generation, revision and exact page-root scope, not UI/font/component state.
pub(crate) fn page_index_cached(state: &EditorState, page: usize) -> Option<Arc<PageIndex>> {
    let roots = PageIndex::roots(state, page)?;
    let key = Key::for_page_index(state, page, roots);
    match get_or_compute_value(shared_store(), Some(key), || {
        Value::PageIndex(PageIndex::build(roots).map(Arc::new))
    }) {
        Value::PageIndex(index) => index,
        _ => unreachable!("page index has an exclusive cache key kind"),
    }
}
