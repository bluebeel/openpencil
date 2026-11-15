//! Exact memoization for one immutable painter call, never across frames.
//! Cull/layer/mask callers keep every existing arithmetic and fallback rule.
use super::{BoundsResult, SceneNode};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    mem::size_of,
};
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 131_072;
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    node: usize,
    basis: [u32; 4],
    root: bool,
    selected: bool,
}
impl Key {
    fn new(node: &SceneNode, root: bool, basis: glam::Mat2, selected: bool) -> Self {
        Self {
            node: std::ptr::from_ref(node) as usize,
            basis: basis.to_cols_array().map(f32::to_bits),
            root,
            selected,
        }
    }
}
#[derive(Default)]
struct Cache {
    values: HashMap<Key, BoundsResult>,
}
impl Cache {
    fn insert(&mut self, key: Key, value: BoundsResult) {
        if self.values.len() >= MAX_ENTRIES {
            return;
        }
        // Predict a growth before asking HashMap to allocate. A completed
        // paint never retains this backing; allocator RSS is measured outside.
        if self.values.len() == self.values.capacity() {
            let capacity = self
                .values
                .capacity()
                .saturating_mul(2)
                .saturating_add(1)
                .max(3);
            let bytes = capacity.saturating_mul(size_of::<(Key, BoundsResult)>() + 16);
            if bytes > MAX_BYTES || self.values.try_reserve(1).is_err() {
                return;
            }
        }
        self.values.insert(key, value);
    }
}
thread_local! {
    static ACTIVE: RefCell<Option<Cache>> = const { RefCell::new(None) };
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}
/// Called only by the borrowed root/selection painter entry points. All scene
/// nodes reachable by that call remain immutably borrowed until it returns.
/// A reentrant/custom public paint disables reuse instead of mixing unrelated
/// temporary node identities into the outer cache. Unwind restores the slot.
pub(crate) fn with_scope<R>(action: impl FnOnce() -> R) -> R {
    let depth = DEPTH.with(|depth| depth.replace(depth.get() + 1));
    let previous = ACTIVE.with(|active| active.replace((depth == 0).then(Cache::default)));
    struct Restore {
        previous: Option<Cache>,
        depth: usize,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            ACTIVE.with(|active| {
                active.replace(self.previous.take());
            });
            DEPTH.with(|depth| depth.set(self.depth));
        }
    }
    let _restore = Restore { previous, depth };
    action()
}
pub(super) fn lookup(
    node: &SceneNode,
    root: bool,
    basis: glam::Mat2,
    selected: bool,
) -> Option<BoundsResult> {
    ACTIVE.with(|active| {
        active
            .borrow()
            .as_ref()?
            .values
            .get(&Key::new(node, root, basis, selected))
            .copied()
    })
}
pub(super) fn insert(
    node: &SceneNode,
    root: bool,
    basis: glam::Mat2,
    selected: bool,
    bounds: BoundsResult,
) {
    ACTIVE.with(|active| {
        if let Some(cache) = active.borrow_mut().as_mut() {
            cache.insert(Key::new(node, root, basis, selected), bounds);
        }
    });
}
