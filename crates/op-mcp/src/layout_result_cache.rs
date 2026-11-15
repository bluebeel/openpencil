//! Repeated trusted layout reads reuse exact encoded output. The geometry Arc
//! already proves theme/font/layout inputs; canonical ownership/revision covers
//! original names/types. Public arbitrary-document factories stay fresh.
use super::*;
use std::{
    collections::VecDeque,
    mem::size_of,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, OnceLock, Weak,
    },
};
const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 128;
static LIVE: AtomicUsize = AtomicUsize::new(0);
struct Charge(usize);
impl Drop for Charge {
    fn drop(&mut self) {
        LIVE.fetch_sub(self.0, Ordering::AcqRel);
    }
}
struct Encoded {
    json: String,
    _charge: Charge,
}
struct Key {
    owner: Weak<()>,
    generation: u64,
    revision: u64,
    geometry: Weak<Vec<ComputedGeometry>>,
    page: String,
    parent: Option<String>,
    depth: u32,
}
impl Key {
    fn matches(
        &self,
        state: &EditorState,
        geometry: &Arc<Vec<ComputedGeometry>>,
        page: &str,
        parent: Option<&str>,
        depth: u32,
    ) -> bool {
        self.owner.ptr_eq(&state.derived_cache_owner())
            && self.generation == state.document_generation()
            && self.revision == state.document_revision()
            && self.geometry.ptr_eq(&Arc::downgrade(geometry))
            && self.page == page
            && self.parent.as_deref() == parent
            && self.depth == depth
    }
}
struct Entry {
    key: Key,
    value: Arc<Encoded>,
}
fn cache() -> &'static Mutex<VecDeque<Entry>> {
    static CACHE: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_ENTRIES)))
}
pub(super) fn result(
    state: &EditorState,
    geometry: &Arc<Vec<ComputedGeometry>>,
    page: &str,
    parent: Option<&str>,
    depth: u32,
    trusted: bool,
    build: impl FnOnce() -> ToolOutcome,
) -> ToolOutcome {
    if !trusted || page.len() > 4096 || parent.is_some_and(|p| p.len() > 4096) {
        return build();
    }
    let mut dead = Vec::new();
    let hit = {
        let mut entries = cache().lock().unwrap_or_else(|e| e.into_inner());
        let mut at = 0;
        while at < entries.len() {
            if entries[at].key.owner.strong_count() == 0
                || entries[at].key.geometry.strong_count() == 0
                || (entries[at].key.owner.ptr_eq(&state.derived_cache_owner())
                    && (entries[at].key.generation != state.document_generation()
                        || entries[at].key.revision != state.document_revision()))
            {
                dead.push(entries.remove(at).unwrap());
            } else {
                at += 1;
            }
        }
        entries
            .iter()
            .position(|e| e.key.matches(state, geometry, page, parent, depth))
            .map(|at| {
                let entry = entries.remove(at).unwrap();
                let value = Arc::clone(&entry.value);
                entries.push_back(entry);
                value
            })
    };
    drop(dead);
    if let Some(hit) = hit {
        return ToolOutcome::OkJson(hit.json.clone());
    }
    let outcome = build();
    let ToolOutcome::OkJson(json) = outcome else {
        return outcome;
    };
    let bytes = json
        .capacity()
        .saturating_add(page.len())
        .saturating_add(parent.map_or(0, str::len))
        .saturating_add(size_of::<Entry>() + size_of::<Encoded>() + 512);
    let reserve = || {
        LIVE.fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
            live.checked_add(bytes)
                .filter(|n| *n <= MAX_BYTES.saturating_sub(MAX_ENTRIES * size_of::<Entry>()))
        })
        .ok()
        .map(|_| Charge(bytes))
    };
    if bytes > MAX_BYTES {
        return ToolOutcome::OkJson(json);
    }
    let mut charge = reserve();
    for _ in 0..MAX_ENTRIES {
        if charge.is_some() {
            break;
        }
        let old = cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front();
        let Some(old) = old else {
            break;
        };
        drop(old);
        charge = reserve();
    }
    let Some(charge) = charge else {
        return ToolOutcome::OkJson(json);
    };
    let encoded = Arc::new(Encoded {
        json,
        _charge: charge,
    });
    let entry = Entry {
        key: Key {
            owner: state.derived_cache_owner(),
            generation: state.document_generation(),
            revision: state.document_revision(),
            geometry: Arc::downgrade(geometry),
            page: page.into(),
            parent: parent.map(str::to_owned),
            depth,
        },
        value: Arc::clone(&encoded),
    };
    let old = {
        let mut entries = cache().lock().unwrap_or_else(|e| e.into_inner());
        let old = (entries.len() >= MAX_ENTRIES)
            .then(|| entries.pop_front())
            .flatten();
        entries.push_back(entry);
        old
    };
    drop(old);
    ToolOutcome::OkJson(encoded.json.clone())
}
