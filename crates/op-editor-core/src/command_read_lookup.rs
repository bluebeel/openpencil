//! Explicit trusted-host read authority. Ordinary public snapshots never opt in.
//! The shared cache owns locations/IDs only, not canonical node payloads or actors.
use super::{lookup, SeedOverride};
use crate::{history::Scope, EditorState, PenNodeExt};
use jian_ops_schema::node::PenNode;
use std::{
    collections::VecDeque,
    mem,
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    sync::{LazyLock, Mutex, Weak},
};
const CACHE_BYTES: usize = 1024 * 1024 * 1024;
const CACHE_FILES: usize = 64;
const COLD_BYTES: usize = 512 * 1024 * 1024;
#[derive(Clone, Copy, Debug)]
pub enum NodeReadOrder {
    First,
    Last,
    /// Legacy get_node_children records parents after their descendants and
    /// ignores empty child lists when overwriting a previous duplicate row.
    LastNonEmptyChildren,
}
pub struct NodeRead<'a> {
    pub node: &'a PenNode,
    pub parent: Option<&'a PenNode>,
    pub depth: usize,
}
pub struct TrustedNodeLookup<'a> {
    state: &'a EditorState,
    roots: &'a [PenNode],
    scope: Scope,
}
struct Record {
    owner: Weak<()>,
    generation: u64,
    revision: u64,
    bytes: usize,
    index: lookup::Index,
}
#[derive(Default)]
struct ReadCache {
    records: VecDeque<Record>,
    bytes: usize,
    active: usize,
}
impl ReadCache {
    fn take(&mut self, state: &EditorState) -> Option<lookup::Index> {
        let owner = state.derived_cache_owner();
        let mut found = None;
        let mut kept = VecDeque::new();
        self.bytes = 0;
        for record in self.records.drain(..) {
            if record.owner.ptr_eq(&owner) {
                if record.generation == state.document_generation()
                    && record.revision == state.document_revision()
                {
                    found = Some(record.index);
                }
            } else if record.owner.strong_count() != 0 {
                self.bytes = self.bytes.saturating_add(record.bytes);
                kept.push_back(record);
            }
        }
        self.records = kept;
        found
    }
    fn acquire(&mut self, state: &EditorState) -> Option<(Option<lookup::Index>, usize)> {
        let index = self.take(state);
        let reservation = index
            .as_ref()
            .map_or(COLD_BYTES, lookup::Index::storage_bytes);
        while self
            .bytes
            .saturating_add(self.active)
            .saturating_add(reservation)
            > CACHE_BYTES
        {
            let Some(old) = self.records.pop_front() else {
                return None;
            };
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        self.active = self.active.saturating_add(reservation);
        Some((index, reservation))
    }
    fn release(&mut self, state: &EditorState, reservation: usize, index: Option<lookup::Index>) {
        self.active = self.active.saturating_sub(reservation);
        let Some(index) = index else {
            return;
        };
        let bytes = index.storage_bytes();
        if bytes > reservation || bytes > CACHE_BYTES {
            return;
        }
        while self.records.len() >= CACHE_FILES
            || self.bytes.saturating_add(self.active).saturating_add(bytes) > CACHE_BYTES
        {
            let Some(old) = self.records.pop_front() else {
                return;
            };
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.records.push_back(Record {
            owner: state.derived_cache_owner(),
            generation: state.document_generation(),
            revision: state.document_revision(),
            bytes,
            index,
        });
    }
}
static CACHE: LazyLock<Mutex<ReadCache>> = LazyLock::new(|| Mutex::new(ReadCache::default()));
impl EditorState {
    /// Only a host holding complete file/editor/actor authority may use this
    /// scope. All prior canonical writes must have advanced normal revision or
    /// generation; public raw-Doc/custom-applier callers must remain uncached.
    /// The immutable callback cannot mutate Doc or return a borrowed registry.
    pub fn with_trusted_read_lookup<R>(&mut self, read: impl FnOnce(&EditorState) -> R) -> R {
        if self.command_id_seed.entered
            && self.command_id_seed.valid
            && self.command_id_seed.generation == self.document_generation()
        {
            // A writer cohort already owns an observed location authority.
            return read(self);
        }
        let generation = self.document_generation();
        let revision = self.document_revision();
        let Some((index, reservation)) =
            CACHE.lock().ok().and_then(|mut cache| cache.acquire(self))
        else {
            return read(self);
        };
        let mut override_ = SeedOverride::new(generation, None, None);
        override_.lookup = Mutex::new(lookup::Cache::bounded(index, reservation));
        let prior = mem::replace(&mut self.command_id_seed, override_);
        let result = catch_unwind(AssertUnwindSafe(|| read(self)));
        let mut observed = mem::replace(&mut self.command_id_seed, prior);
        let index = if result.is_ok()
            && self.document_generation() == generation
            && self.document_revision() == revision
        {
            observed.take_lookup(self)
        } else {
            None
        };
        // Destroy a panicked/invalid active index before returning its quota.
        drop(observed);
        if let Ok(mut cache) = CACHE.lock() {
            cache.release(self, reservation, index);
        }
        match result {
            Ok(result) => result,
            Err(panic) => resume_unwind(panic),
        }
    }
    /// Proof-bearing access only while an explicit locked read/write scope is
    /// active, and only for the real complete root/page forest being borrowed.
    pub fn trusted_node_lookup<'a>(
        &'a self,
        roots: &'a [PenNode],
    ) -> Option<TrustedNodeLookup<'a>> {
        let seed = &self.command_id_seed;
        if !seed.entered || !seed.valid || seed.generation != self.document_generation() {
            return None;
        }
        let same = |a: &[PenNode]| a.len() == roots.len() && a.as_ptr() == roots.as_ptr();
        let scope = if same(&self.doc.children) {
            Scope::Root
        } else {
            let (index, page) = self
                .doc
                .pages
                .as_ref()?
                .iter()
                .enumerate()
                .find(|(_, page)| same(&page.children))?;
            Scope::Page {
                index,
                id: page.id.clone(),
            }
        };
        Some(TrustedNodeLookup {
            state: self,
            roots,
            scope,
        })
    }
}
impl<'a> TrustedNodeLookup<'a> {
    /// False allows multi-ID callers to keep one fresh forest walk when the
    /// bounded index cannot be built, rather than scanning once per ID.
    pub fn is_indexed(&self) -> bool {
        self.state
            .command_id_seed
            .lookup
            .lock()
            .ok()
            .is_some_and(|mut cache| cache.ready(self.state))
    }
    pub fn find(&self, id: &str, order: NodeReadOrder) -> Option<NodeRead<'a>> {
        let mode = match order {
            NodeReadOrder::First => lookup::Order::Preorder,
            NodeReadOrder::Last => lookup::Order::LastPreorder,
            NodeReadOrder::LastNonEmptyChildren => lookup::Order::LastNonEmptyPostorder,
        };
        let path = self
            .state
            .command_id_seed
            .lookup
            .lock()
            .ok()
            .and_then(|mut cache| cache.path(self.state, &self.scope, id, mode));
        if let Some(path) = path {
            let path = path?;
            let mut siblings = self.roots;
            let mut parent = None;
            for (depth, slot) in path.iter().enumerate() {
                let Some(node) = siblings.get(*slot) else {
                    break;
                };
                if depth + 1 == path.len() && node.id_str() == id {
                    return Some(NodeRead {
                        node,
                        parent,
                        depth,
                    });
                }
                parent = Some(node);
                siblings = node.children().map_or(&[], Vec::as_slice);
            }
        }
        // A pending/unsupported structural command invalidates route reuse;
        // an immutable read can still use the exact fresh legacy traversal.
        fresh(self.roots, None, 0, id, order)
    }
}
fn fresh<'a>(
    nodes: &'a [PenNode],
    parent: Option<&'a PenNode>,
    depth: usize,
    id: &str,
    order: NodeReadOrder,
) -> Option<NodeRead<'a>> {
    let mut found = None;
    for node in nodes {
        if matches!(order, NodeReadOrder::LastNonEmptyChildren) {
            if let Some(children) = node.children().filter(|children| !children.is_empty()) {
                if let Some(result) = fresh(children, Some(node), depth + 1, id, order) {
                    found = Some(result);
                }
                if node.id_str() == id {
                    found = Some(NodeRead {
                        node,
                        parent,
                        depth,
                    });
                }
            }
        } else {
            if node.id_str() == id {
                let result = NodeRead {
                    node,
                    parent,
                    depth,
                };
                if matches!(order, NodeReadOrder::First) {
                    return Some(result);
                }
                found = Some(result);
            }
            if let Some(result) = node
                .children()
                .and_then(|children| fresh(children, Some(node), depth + 1, id, order))
            {
                if matches!(order, NodeReadOrder::First) {
                    return Some(result);
                }
                found = Some(result);
            }
        }
    }
    found
}
