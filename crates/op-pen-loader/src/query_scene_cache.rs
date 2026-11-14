//! Explicit trusted-authority reuse of immutable interactive scenes/query indexes.
//! Fresh public builders never enter this cache. Only metadata is globally locked;
//! same-key builds coalesce, unrelated documents build on their calling threads.
use crate::scene_inputs::SceneInputs;
use jian_scene::{layout_scene::LayoutScene, snapping::SnapIndex};
use op_editor_core::EditorState;
use std::{
    mem::size_of,
    sync::{Arc, Condvar, Mutex, OnceLock, Weak},
};

#[path = "query_scene_storage.rs"]
mod storage;

// Large imported pages must participate in reuse too. The user's current
// working ceiling is5GB;1GB remains a measured engine optimization target.
pub const MAX_QUERY_SCENE_CACHE_BYTES: usize = 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 64;
const MAX_BUILDS: usize = 8;
const MAX_KEY_BYTES: usize = 4 * 1024 * 1024;

/// Conservative live retained capacity, including evicted values held by callers.
/// Builds, fresh/oversized bypass results, allocator/native retention and caller
/// response buffers are separate transient work, not hidden in this cache bound.
#[derive(Clone, Copy, Debug, Default)]
pub struct QuerySceneCacheStats {
    pub retained_entries: usize,
    pub retired_pinned: usize,
    pub building: usize,
    pub charged_bytes: usize,
    pub hits: u64,
    pub builds: u64,
    pub coalesced: u64,
    pub uncached_builds: u64,
}

#[path = "export_scene_cache.rs"]
mod exports;
pub use exports::{
    prepare_export_scene_cached, CachedExportScene, ExportScenePolicy, ExportSceneUnavailable,
};

enum Key {
    Scene(SceneInputs),
    Export {
        input: SceneInputs,
        policy: ExportScenePolicy,
        selected: Option<String>,
    },
    Snap {
        scene: Weak<LayoutScene>,
        parent: Option<String>,
    },
}
impl Key {
    fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Scene(a), Self::Scene(b)) => a.matches(b),
            (
                Self::Export {
                    input: a,
                    policy: pa,
                    selected: sa,
                },
                Self::Export {
                    input: b,
                    policy: pb,
                    selected: sb,
                },
            ) => a.matches(b) && pa == pb && sa == sb,
            (
                Self::Snap {
                    scene: a,
                    parent: pa,
                },
                Self::Snap {
                    scene: b,
                    parent: pb,
                },
            ) => Weak::ptr_eq(a, b) && pa == pb,
            _ => false,
        }
    }
    fn live(&self) -> bool {
        match self {
            Self::Scene(input) | Self::Export { input, .. } => input.owner_live(),
            Self::Snap { scene, .. } => scene.strong_count() != 0,
        }
    }
    fn stable(&self) -> bool {
        match self {
            Self::Scene(input) | Self::Export { input, .. } => input.fonts_stable(),
            _ => true,
        }
    }
    fn bytes(&self) -> usize {
        size_of::<Self>()
            + 32
            + match self {
                Self::Scene(input) => storage::input_bytes(input),
                Self::Export {
                    input,
                    policy,
                    selected,
                } => {
                    storage::input_bytes(input)
                        + policy.bytes()
                        + selected.as_ref().map_or(0, String::capacity)
                }
                Self::Snap { parent, .. } => parent.as_ref().map_or(0, String::capacity),
            }
    }
}
#[derive(Clone)]
enum Value {
    Scene(Arc<LayoutScene>),
    Snap(Option<Arc<SnapIndex>>),
}
impl Value {
    fn bytes(&self) -> usize {
        32usize.saturating_add(match self {
            Self::Scene(scene) => storage::scene_bytes(scene),
            Self::Snap(index) => index.as_ref().map_or(0, |index| index.storage_bytes()),
        })
    }
    fn weak(&self) -> WeakValue {
        match self {
            Self::Scene(scene) => WeakValue::Scene(Arc::downgrade(scene)),
            Self::Snap(index) => WeakValue::Snap(index.as_ref().map(Arc::downgrade)),
        }
    }
}
#[derive(Clone)]
enum WeakValue {
    Scene(Weak<LayoutScene>),
    Snap(Option<Weak<SnapIndex>>),
}
impl WeakValue {
    fn live(&self) -> bool {
        match self {
            Self::Scene(value) => value.strong_count() != 0,
            Self::Snap(value) => value
                .as_ref()
                .is_some_and(|value| value.strong_count() != 0),
        }
    }
}
struct Entry {
    key: Arc<Key>,
    value: Value,
    bytes: usize,
    value_bytes: usize,
}
struct Retired {
    value: WeakValue,
    bytes: usize,
}
enum BuildState {
    Building,
    Ready(Value),
    Failed,
}
struct BuildSlot {
    key: Arc<Key>,
    state: Mutex<BuildState>,
    wake: Condvar,
}
impl BuildSlot {
    fn usable(&self) -> bool {
        !matches!(
            *self.state.lock().unwrap_or_else(|e| e.into_inner()),
            BuildState::Failed
        )
    }
    fn wait(&self) -> Option<Value> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match &*state {
                BuildState::Building => {
                    state = self.wake.wait(state).unwrap_or_else(|e| e.into_inner())
                }
                BuildState::Ready(value) => return Some(value.clone()),
                BuildState::Failed => return None,
            }
        }
    }
    fn publish(&self, value: Value) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = BuildState::Ready(value);
        self.wake.notify_all();
    }
}
struct BuildGuard {
    slot: Arc<BuildSlot>,
    finished: bool,
}
impl Drop for BuildGuard {
    fn drop(&mut self) {
        if !self.finished {
            *self.slot.state.lock().unwrap_or_else(|e| e.into_inner()) = BuildState::Failed;
            self.slot.wake.notify_all();
        }
    }
}
struct Pending {
    slot: Weak<BuildSlot>,
    key_bytes: usize,
}
struct Store {
    entries: Vec<Entry>,
    retired: Vec<Retired>,
    pending: Vec<Pending>,
    bytes: usize,
    hits: u64,
    builds: u64,
    coalesced: u64,
    uncached: u64,
}
impl Store {
    fn new() -> Self {
        Self {
            entries: Vec::with_capacity(MAX_ENTRIES),
            retired: Vec::with_capacity(MAX_ENTRIES),
            pending: Vec::with_capacity(MAX_BUILDS),
            bytes: 0,
            hits: 0,
            builds: 0,
            coalesced: 0,
            uncached: 0,
        }
    }
    fn base_bytes(&self) -> usize {
        size_of::<Self>()
            + self.entries.capacity() * size_of::<Entry>()
            + self.retired.capacity() * size_of::<Retired>()
            + self.pending.capacity() * size_of::<Pending>()
    }
    fn pending_bytes(&self) -> usize {
        self.pending
            .iter()
            .map(|slot| size_of::<BuildSlot>() + 32 + slot.key_bytes)
            .sum()
    }
    fn charged_bytes(&self) -> usize {
        self.bytes
            .saturating_add(self.base_bytes())
            .saturating_add(self.pending_bytes())
    }
    fn retire(&mut self, index: usize) -> Entry {
        let entry = self.entries.remove(index);
        self.bytes -= entry.bytes - entry.value_bytes;
        self.retired.push(Retired {
            value: entry.value.weak(),
            bytes: entry.value_bytes,
        });
        entry
    }
    fn purge(&mut self, dead: &mut Vec<Entry>, slots: &mut Vec<Arc<BuildSlot>>) {
        let bytes = &mut self.bytes;
        self.retired.retain(|entry| {
            if entry.value.live() {
                true
            } else {
                *bytes -= entry.bytes;
                false
            }
        });
        self.pending.retain(|pending| match pending.slot.upgrade() {
            Some(slot) => {
                let keep = slot.usable();
                slots.push(slot);
                keep
            }
            None => false,
        });
        let mut index = 0;
        while index < self.entries.len() {
            if !self.entries[index].key.live() {
                dead.push(self.retire(index));
            } else {
                index += 1;
            }
        }
    }
    fn pending_hit(&self, key: &Key, slots: &mut Vec<Arc<BuildSlot>>) -> Option<Arc<BuildSlot>> {
        for pending in &self.pending {
            if let Some(slot) = pending.slot.upgrade() {
                if slot.key.matches(key) {
                    return Some(slot);
                }
                slots.push(slot);
            }
        }
        None
    }
    fn hit(&mut self, key: &Key) -> Option<Value> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.key.matches(key))?;
        let entry = self.entries.remove(index);
        let value = entry.value.clone();
        self.entries.push(entry);
        self.hits = self.hits.saturating_add(1);
        Some(value)
    }
}
fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Store::new()))
}

/// No compute or destruction of a large scene happens under the shared lock.
fn get_or_build(key: Option<Key>, compute: impl FnOnce() -> Value) -> Value {
    let Some(key) = key else {
        record_uncached();
        return compute();
    };
    if key.bytes() > MAX_KEY_BYTES {
        record_uncached();
        return compute();
    }
    let key = Arc::new(key);
    let mut compute = Some(compute);
    for _ in 0..MAX_ENTRIES {
        let mut dead = Vec::new();
        let mut slots = Vec::new();
        enum Choice {
            Hit(Value),
            Wait(Arc<BuildSlot>),
            Build(Arc<BuildSlot>),
            Fresh,
        }
        let choice = {
            let mut cache = store().lock().unwrap_or_else(|e| e.into_inner());
            cache.purge(&mut dead, &mut slots);
            if let Some(value) = cache.hit(&key) {
                Choice::Hit(value)
            } else if let Some(slot) = cache.pending_hit(&key, &mut slots) {
                cache.coalesced = cache.coalesced.saturating_add(1);
                Choice::Wait(slot)
            } else if cache.pending.len() < MAX_BUILDS
                && cache
                    .charged_bytes()
                    .saturating_add(key.bytes() + size_of::<BuildSlot>() + 32)
                    <= MAX_QUERY_SCENE_CACHE_BYTES
            {
                let slot = Arc::new(BuildSlot {
                    key: Arc::clone(&key),
                    state: Mutex::new(BuildState::Building),
                    wake: Condvar::new(),
                });
                cache.pending.push(Pending {
                    slot: Arc::downgrade(&slot),
                    key_bytes: key.bytes(),
                });
                cache.builds = cache.builds.saturating_add(1);
                Choice::Build(slot)
            } else {
                cache.uncached = cache.uncached.saturating_add(1);
                Choice::Fresh
            }
        };
        drop(dead);
        drop(slots);
        match choice {
            Choice::Hit(value) => return value,
            Choice::Wait(slot) => {
                if let Some(value) = slot.wait() {
                    return value;
                }
            }
            Choice::Fresh => return compute.take().expect("compute runs once")(),
            Choice::Build(slot) => {
                let mut guard = BuildGuard {
                    slot,
                    finished: false,
                };
                let value = compute.take().expect("compute runs once")();
                if !admit(Arc::clone(&key), &value) {
                    record_uncached();
                }
                guard.slot.publish(value.clone());
                guard.finished = true;
                return value;
            }
        }
    }
    record_uncached();
    compute.take().expect("compute runs once")()
}
fn record_uncached() {
    let mut cache = store().lock().unwrap_or_else(|e| e.into_inner());
    cache.uncached = cache.uncached.saturating_add(1);
}
fn admit(key: Arc<Key>, value: &Value) -> bool {
    let value_bytes = value.bytes();
    let bytes = key.bytes().saturating_add(value_bytes);
    if !key.stable() || bytes > MAX_QUERY_SCENE_CACHE_BYTES {
        return false;
    }
    for _ in 0..=MAX_ENTRIES {
        let mut dead = Vec::new();
        let mut slots = Vec::new();
        let evicted = {
            let mut cache = store().lock().unwrap_or_else(|e| e.into_inner());
            cache.purge(&mut dead, &mut slots);
            if cache.entries.len() + cache.retired.len() < MAX_ENTRIES
                && cache.charged_bytes().saturating_add(bytes) <= MAX_QUERY_SCENE_CACHE_BYTES
            {
                cache.bytes += bytes;
                cache.entries.push(Entry {
                    key,
                    value: value.clone(),
                    bytes,
                    value_bytes,
                });
                drop(cache);
                drop(dead);
                drop(slots);
                return true;
            }
            if cache.entries.is_empty() {
                None
            } else {
                Some(cache.retire(0))
            }
        };
        let exhausted = evicted.is_none();
        drop(dead);
        drop(slots);
        drop(evicted);
        if exhausted {
            return false;
        }
    }
    false
}

/// Trusted hosts only: every canonical edit/reload must maintain revision and
/// generation authority. Direct-field-mutating/public snapshot callers remain fresh.
pub fn active_query_scene_cached(state: &EditorState) -> Arc<LayoutScene> {
    let key = SceneInputs::capture(state).map(Key::Scene);
    match get_or_build(key, || {
        Value::Scene(Arc::new(crate::editor_state_to_active_page_layout_scene(
            state,
        )))
    }) {
        Value::Scene(scene) => scene,
        Value::Snap(_) => unreachable!("cache key kind"),
    }
}
/// Reuse the real editor SnapIndex for this exact immutable scene allocation.
/// This key cannot alias a different scene even after an editor revision expires.
pub fn query_snap_index_cached(
    scene: &Arc<LayoutScene>,
    parent: Option<&str>,
) -> Option<Arc<SnapIndex>> {
    let key = parent.is_none_or(|id| id.len() <= 4096).then(|| Key::Snap {
        scene: Arc::downgrade(scene),
        parent: parent.map(str::to_owned),
    });
    match get_or_build(key, || {
        Value::Snap(SnapIndex::for_scene(scene, parent).map(Arc::new))
    }) {
        Value::Snap(index) => index,
        Value::Scene(_) => unreachable!("cache key kind"),
    }
}
pub fn query_scene_cache_stats() -> QuerySceneCacheStats {
    let mut dead = Vec::new();
    let mut slots = Vec::new();
    let stats = {
        let mut cache = store().lock().unwrap_or_else(|e| e.into_inner());
        cache.purge(&mut dead, &mut slots);
        QuerySceneCacheStats {
            retained_entries: cache.entries.len(),
            retired_pinned: cache.retired.len(),
            building: cache.pending.len(),
            charged_bytes: cache.charged_bytes(),
            hits: cache.hits,
            builds: cache.builds,
            coalesced: cache.coalesced,
            uncached_builds: cache.uncached,
        }
    };
    drop(dead);
    drop(slots);
    stats
}
