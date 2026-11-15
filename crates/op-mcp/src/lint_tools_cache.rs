//! Shared encoded pure results, never canonical Doc/image/actor copies. A live
//! permit charges evicted-but-still-referenced cached results until final drop.
use super::Prepared;
use op_editor_core::EditorState;
use std::{
    collections::VecDeque,
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
pub(super) struct ResultValue {
    pub(super) result: Prepared,
    _charge: Option<Charge>,
}
struct Entry {
    owner: Weak<()>,
    generation: u64,
    revision: u64,
    filter: Option<String>,
    value: Arc<ResultValue>,
}
impl Entry {
    fn matches(&self, state: &EditorState, filter: Option<&str>) -> bool {
        self.owner.ptr_eq(&state.derived_cache_owner())
            && self.generation == state.document_generation()
            && self.revision == state.document_revision()
            && self.filter.as_deref() == filter
    }
}
fn reserve(bytes: usize) -> Option<Charge> {
    LIVE.fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
        live.checked_add(bytes).filter(|sum| *sum <= MAX_BYTES)
    })
    .ok()
    .map(|_| Charge(bytes))
}
fn store() -> &'static Mutex<VecDeque<Entry>> {
    static CACHE: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_ENTRIES)))
}
fn clean(entries: &mut VecDeque<Entry>, state: &EditorState) {
    let owner = state.derived_cache_owner();
    entries.retain(|entry| {
        entry.owner.strong_count() != 0
            && (!entry.owner.ptr_eq(&owner)
                || (entry.generation == state.document_generation()
                    && entry.revision == state.document_revision()))
    });
}
pub(super) fn result(
    state: &EditorState,
    filter: Option<&str>,
    trusted: bool,
    build: impl FnOnce() -> Prepared,
) -> Arc<ResultValue> {
    let cacheable = trusted && filter.is_none_or(|filter| filter.len() <= 4096);
    if cacheable {
        let mut entries = store().lock().unwrap_or_else(|p| p.into_inner());
        clean(&mut entries, state);
        if let Some(index) = entries
            .iter()
            .position(|entry| entry.matches(state, filter))
        {
            let entry = entries.remove(index).unwrap();
            let value = Arc::clone(&entry.value);
            entries.push_back(entry);
            return value;
        }
    }
    let result = build();
    if !cacheable {
        return Arc::new(ResultValue {
            result,
            _charge: None,
        });
    }
    let bytes = match &result {
        Ok(value) | Err(value) => value.capacity(),
    }
    .saturating_add(filter.map_or(0, str::len))
    .saturating_add(std::mem::size_of::<Entry>())
    .saturating_add(std::mem::size_of::<ResultValue>() + 4 * std::mem::size_of::<usize>());
    if bytes > MAX_BYTES {
        return Arc::new(ResultValue {
            result,
            _charge: None,
        });
    }
    let mut entries = store().lock().unwrap_or_else(|p| p.into_inner());
    clean(&mut entries, state);
    if let Some(entry) = entries.iter().find(|entry| entry.matches(state, filter)) {
        return Arc::clone(&entry.value);
    }
    while entries.len() >= MAX_ENTRIES {
        entries.pop_front();
    }
    let charge = loop {
        if let Some(charge) = reserve(bytes) {
            break Some(charge);
        }
        if entries.pop_front().is_none() {
            break None;
        }
    };
    let tracked = charge.is_some();
    let value = Arc::new(ResultValue {
        result,
        _charge: charge,
    });
    if tracked {
        entries.push_back(Entry {
            owner: state.derived_cache_owner(),
            generation: state.document_generation(),
            revision: state.document_revision(),
            filter: filter.map(str::to_owned),
            value: Arc::clone(&value),
        });
    }
    value
}
