//! Ordered immutable resource-ID plans for shared-worker scene branches.
//! Readiness is never cached: decoded sharpness and encoded bytes are checked
//! at every export, so eviction/loading retain their existing semantics.
use op_editor_ui::layout_scene::{
    stable_image_source_id, LayoutScene, NodeKind, SceneFillLayer, SceneNode,
};
use std::{
    cell::RefCell,
    mem::size_of,
    sync::{Arc, Weak},
};

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_ENTRIES: usize = 128;
const MAX_IDS: usize = 32768;
const ENTRY_ALLOWANCE: usize = 256;

pub(super) enum Plan {
    Known(Box<[u64]>),
    Legacy,
}
impl Plan {
    pub(super) fn ids(&self) -> Option<&[u64]> {
        match self {
            Self::Known(ids) => Some(ids),
            Self::Legacy => None,
        }
    }
    fn bytes(&self) -> usize {
        size_of::<Self>()
            + match self {
                Self::Known(ids) => ids.len() * size_of::<u64>(),
                Self::Legacy => 0,
            }
    }
}
struct Entry {
    owner: Weak<LayoutScene>,
    node: usize,
    plan: Arc<Plan>,
    bytes: usize,
}
#[derive(Default)]
struct Cache {
    entries: Vec<Entry>,
    payload: usize,
}
impl Cache {
    fn bytes(&self) -> usize {
        self.entries.capacity() * size_of::<Entry>() + self.payload
    }
    fn purge(&mut self) {
        let mut at = 0;
        while at < self.entries.len() {
            if self.entries[at].owner.strong_count() == 0 {
                self.payload -= self.entries.remove(at).bytes;
            } else {
                at += 1;
            }
        }
    }
    fn find(&mut self, owner: &Weak<LayoutScene>, node: usize) -> Option<Arc<Plan>> {
        self.purge();
        let at = self
            .entries
            .iter()
            .position(|entry| entry.node == node && Weak::ptr_eq(owner, &entry.owner))?;
        let entry = self.entries.remove(at);
        let plan = Arc::clone(&entry.plan);
        self.entries.push(entry);
        Some(plan)
    }
    fn insert(&mut self, owner: Weak<LayoutScene>, node: usize, plan: Arc<Plan>) {
        let bytes = plan.bytes() + ENTRY_ALLOWANCE;
        let container = MAX_ENTRIES * size_of::<Entry>();
        if bytes > MAX_BYTES.saturating_sub(container) {
            return;
        }
        if self.entries.capacity() == 0 && self.entries.try_reserve_exact(MAX_ENTRIES).is_err() {
            return;
        }
        while (self.bytes() > MAX_BYTES.saturating_sub(bytes) || self.entries.len() == MAX_ENTRIES)
            && !self.entries.is_empty()
        {
            self.payload -= self.entries.remove(0).bytes;
        }
        if self.bytes() > MAX_BYTES.saturating_sub(bytes) {
            return;
        }
        self.entries.push(Entry {
            owner,
            node,
            plan,
            bytes,
        });
        self.payload += bytes;
    }
}
thread_local! {
    static OWNER: RefCell<Option<Weak<LayoutScene>>> = const { RefCell::new(None) };
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

/// Private shared-worker boundary: every preparation node resolved by the
/// enclosed product job is borrowed from this exact immutable Arc. Public raw
/// canvas/custom callbacks never install this authority. No scene is retained.
pub(super) fn with_scene<R>(scene: &Arc<LayoutScene>, action: impl FnOnce() -> R) -> R {
    struct Restore(Option<Weak<LayoutScene>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OWNER.with(|owner| {
                owner.replace(self.0.take());
            });
        }
    }
    let previous = OWNER.with(|owner| owner.replace(Some(Arc::downgrade(scene))));
    let _restore = Restore(previous);
    action()
}
pub(super) fn for_node(node: &SceneNode) -> Option<Arc<Plan>> {
    let owner = OWNER.with(|owner| owner.borrow().clone())?;
    if owner.strong_count() == 0 {
        return None;
    }
    let key = std::ptr::from_ref(node) as usize;
    if let Some(plan) = CACHE.with(|cache| cache.borrow_mut().find(&owner, key)) {
        return Some(plan);
    }
    let plan = Arc::new(build(node));
    CACHE.with(|cache| cache.borrow_mut().insert(owner, key, Arc::clone(&plan)));
    Some(plan)
}
fn build(root: &SceneNode) -> Plan {
    let mut ids = Vec::new();
    let mut stack = vec![std::slice::from_ref(root).iter()];
    let mut push = |id: u64, source: &str| {
        if ids.len() == MAX_IDS {
            return false;
        }
        ids.push(if id == 0 {
            stable_image_source_id(source)
        } else {
            id
        });
        true
    };
    while let Some(siblings) = stack.last_mut() {
        let Some(node) = siblings.next() else {
            stack.pop();
            continue;
        };
        if node.widget.is_some() || matches!(node.kind, NodeKind::Other(_)) {
            return Plan::Legacy;
        }
        for (source, id) in [
            (node.image_src.as_deref(), node.image_src_id),
            (
                node.image_thumbnail_src.as_deref(),
                node.image_thumbnail_src_id,
            ),
        ] {
            if let Some(source) = source {
                if !push(id, source) {
                    return Plan::Legacy;
                }
            }
        }
        for layer in node.fill_layers.iter().chain(
            node.stroke_paint
                .iter()
                .flat_map(|stroke| stroke.layers.iter()),
        ) {
            if let SceneFillLayer::Image {
                src,
                src_id,
                thumbnail_src,
                thumbnail_src_id,
                ..
            } = layer
            {
                if !push(*src_id, src) {
                    return Plan::Legacy;
                }
                if let Some(source) = thumbnail_src {
                    if !push(*thumbnail_src_id, source) {
                        return Plan::Legacy;
                    }
                }
            }
        }
        if !node.children.is_empty() {
            stack.push(node.children.iter());
        }
    }
    // Keep duplicate IDs and their source traversal order. Encoded-byte cache
    // access can update LRU state; deduplicating checks would alter that order.
    Plan::Known(ids.into_boxed_slice())
}
