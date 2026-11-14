//! Exact point-mode geometry preparation, shared across cameras and exports.
//! Cached paths own no font, paint shader, pixel buffers or document/revision.
use skia_safe::{Path, Rect};
use std::{cell::RefCell, mem::size_of};

const MAX_ENTRIES: usize = 128;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 256 * 1024;
const ENTRY_ALLOWANCE: usize = 512;
const CONTAINER_ALLOWANCE: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Key {
    Transform {
        matrix: [u32; 9],
        even_odd: bool,
    },
    Fit {
        bounds: [u32; 4],
        even_odd: bool,
    },
    Stroke {
        matrix: [u32; 9],
        style: u32,
        width: u32,
        miter: u32,
        cap: u32,
        join: u32,
    },
    TightBounds,
}
#[derive(Clone)]
pub(super) enum Value {
    Path(Path),
    Bounds(Rect),
}
impl Value {
    fn bytes(&self) -> usize {
        match self {
            Self::Path(path) => path.approximate_bytes_used(),
            Self::Bounds(_) => 0,
        }
    }
}
struct Entry {
    original: Path,
    key: Key,
    value: Value,
    bytes: usize,
}
#[derive(Default)]
struct Cache {
    entries: Vec<Entry>,
    payload: usize,
}
impl Cache {
    fn bytes(&self) -> usize {
        if self.entries.capacity() == 0 {
            0
        } else {
            CONTAINER_ALLOWANCE + self.entries.capacity() * size_of::<Entry>() + self.payload
        }
    }
    fn trim(&mut self, budget: usize) {
        while self.bytes() > budget && !self.entries.is_empty() {
            self.payload -= self.entries.remove(0).bytes;
        }
        if self.entries.is_empty() && self.bytes() > budget {
            self.entries = Vec::new();
        }
    }
    fn find(&mut self, path: &Path, key: Key) -> Option<Value> {
        // Compare exact source bits, not only Skia numeric equality (which
        // need not distinguish signed zero), a generation, or a digest. Equal
        // freshly rebuilt icons can reuse geometry without serializing a key.
        let at = self
            .entries
            .iter()
            .position(|entry| entry.key == key && exact_path(&entry.original, path))?;
        let entry = self.entries.remove(at);
        let value = entry.value.clone();
        self.entries.push(entry);
        Some(value)
    }
    fn insert(&mut self, path: &Path, key: Key, value: Value, budget: usize) {
        let bytes = ENTRY_ALLOWANCE.saturating_add(
            2usize.saturating_mul(path.approximate_bytes_used().saturating_add(value.bytes())),
        );
        let container = CONTAINER_ALLOWANCE + MAX_ENTRIES * size_of::<Entry>();
        if bytes > MAX_ENTRY_BYTES || bytes > budget.saturating_sub(container) {
            return;
        }
        if self.entries.capacity() == 0 && self.entries.try_reserve_exact(MAX_ENTRIES).is_err() {
            return;
        }
        self.trim(budget.saturating_sub(bytes));
        if self.entries.capacity() == 0 || self.bytes() > budget.saturating_sub(bytes) {
            return;
        }
        if self.entries.len() == MAX_ENTRIES {
            self.payload -= self.entries.remove(0).bytes;
        }
        self.entries.push(Entry {
            original: path.clone(),
            key,
            value,
            bytes,
        });
        self.payload += bytes;
    }
}
fn exact_path(left: &Path, right: &Path) -> bool {
    left.fill_type() == right.fill_type()
        && left.verbs() == right.verbs()
        && left.points().len() == right.points().len()
        && left.conic_weights().len() == right.conic_weights().len()
        && left
            .points()
            .iter()
            .zip(right.points())
            .all(|(a, b)| a.x.to_bits() == b.x.to_bits() && a.y.to_bits() == b.y.to_bits())
        && left
            .conic_weights()
            .iter()
            .zip(right.conic_weights())
            .all(|(a, b)| a.to_bits() == b.to_bits())
}
thread_local! { static CACHE: RefCell<Cache> = RefCell::new(Cache::default()); }

pub(super) fn retained_bytes() -> usize {
    CACHE.with(|cache| cache.borrow().bytes())
}
pub(super) fn trim(budget: usize) {
    CACHE.with(|cache| cache.borrow_mut().trim(budget.min(MAX_BYTES)));
}
pub(super) fn get_or_build(
    path: &Path,
    key: Key,
    build: impl FnOnce() -> Option<Value>,
) -> Option<Value> {
    if path.approximate_bytes_used() > MAX_ENTRY_BYTES / 2 {
        return build();
    }
    let budget = super::request_headroom().min(MAX_BYTES);
    let found = CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.trim(budget);
        cache.find(path, key)
    });
    if found.is_some() {
        return found;
    }
    // Native work and callback execution never run under a cache borrow. Only
    // successful geometry is retained; failures keep the original retry path.
    let value = build()?;
    CACHE.with(|cache| {
        cache.borrow_mut().insert(
            path,
            key,
            value.clone(),
            super::request_headroom().min(MAX_BYTES),
        )
    });
    // Enforce shared8MiB immediately when this tier grows, not at the next
    // mask lookup. Evicted values pinned by a current draw are active working
    // storage, just as escaped mask shaders are, not retained-cache payload.
    let mask_budget = super::request_headroom().saturating_sub(retained_bytes());
    super::super::stencil::trim_retained_cache(mask_budget);
    Some(value)
}
