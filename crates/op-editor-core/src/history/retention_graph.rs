//! Incremental union of immutable history allocations, never strong owners.
//!
//! Each distinct edge allocation contributes one set of outgoing references.
//! Adding/removing a checkpoint therefore visits only newly live/dead edges,
//! rather than every leaf of every retained checkpoint on every request.
use crate::{
    history::History,
    history_snapshot::{command_capture_node_bytes, command_capture_value_bytes, CaptureSizeError},
    PenDocument, ScopedEditorSnapshot, SharedComponent, SharedNode,
};
use std::{
    collections::{HashMap, HashSet},
    mem::{size_of, size_of_val},
    sync::{Arc, Weak},
};

type Nodes = Vec<Arc<SharedNode>>;
type Pages = Vec<Arc<Nodes>>;
type Components = Vec<Arc<SharedComponent>>;
const MAX_ENTRIES: usize = 1_048_576;
const MAX_BYTES: usize = 256 * 1024 * 1024;
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    pointer: usize,
    kind: u8,
}
#[derive(Clone, Copy)]
enum Value<'a> {
    Document(&'a Arc<PenDocument>, usize),
    Node(&'a Arc<SharedNode>),
    Nodes(&'a Arc<Nodes>),
    Pages(&'a Arc<Pages>),
    Component(&'a Arc<SharedComponent>),
    Components(&'a Arc<Components>),
    Scoped(&'a Arc<ScopedEditorSnapshot>),
}
// A weak control block prevents an address from identifying another allocation
// while cached. Arc::make_mut also detaches weak-observed metadata/edge arrays.
#[allow(dead_code)]
enum Owner {
    Document(Weak<PenDocument>),
    Node(Weak<SharedNode>),
    Nodes(Weak<Nodes>),
    Pages(Weak<Pages>),
    Component(Weak<SharedComponent>),
    Components(Weak<Components>),
    Scoped(Weak<ScopedEditorSnapshot>),
}
struct Entry {
    _owner: Owner,
    references: usize,
    charge: usize,
    children: Vec<Key>,
}
pub(super) struct HistoryGraphCache {
    entries: HashMap<Key, Entry>,
    roots: HashSet<Key>,
    total: usize,
    child_bytes: usize,
    pub(super) additions: usize,
    max_bytes: usize,
    max_entries: usize,
}
impl Default for HistoryGraphCache {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            roots: HashSet::new(),
            total: 0,
            child_bytes: 0,
            additions: 0,
            max_bytes: MAX_BYTES,
            max_entries: MAX_ENTRIES,
        }
    }
}
enum Error {
    Capacity,
    Charge(CaptureSizeError),
}
impl From<CaptureSizeError> for Error {
    fn from(e: CaptureSizeError) -> Self {
        Self::Charge(e)
    }
}
impl Value<'_> {
    fn key(self) -> Key {
        let (pointer, kind) = match self {
            Self::Document(v, _) => (Arc::as_ptr(v) as usize, 0),
            Self::Node(v) => (Arc::as_ptr(v) as usize, 1),
            Self::Nodes(v) => (Arc::as_ptr(v) as usize, 2),
            Self::Pages(v) => (Arc::as_ptr(v) as usize, 3),
            Self::Component(v) => (Arc::as_ptr(v) as usize, 4),
            Self::Components(v) => (Arc::as_ptr(v) as usize, 5),
            Self::Scoped(v) => (Arc::as_ptr(v) as usize, 6),
        };
        Key { pointer, kind }
    }
    fn owner(self) -> Owner {
        match self {
            Self::Document(v, _) => Owner::Document(Arc::downgrade(v)),
            Self::Node(v) => Owner::Node(Arc::downgrade(v)),
            Self::Nodes(v) => Owner::Nodes(Arc::downgrade(v)),
            Self::Pages(v) => Owner::Pages(Arc::downgrade(v)),
            Self::Component(v) => Owner::Component(Arc::downgrade(v)),
            Self::Components(v) => Owner::Components(Arc::downgrade(v)),
            Self::Scoped(v) => Owner::Scoped(Arc::downgrade(v)),
        }
    }
    fn child_count(self) -> usize {
        match self {
            Self::Document(_, _) => 0,
            Self::Node(v) => usize::from(v.children_arc().is_some()),
            Self::Nodes(v) => v.len(),
            Self::Pages(v) => v.len(),
            Self::Component(_) => 1,
            Self::Components(v) => v.len(),
            Self::Scoped(v) => v.retained_roots().count() + 1,
        }
    }
    fn visit(self, mut visit: impl FnMut(Value<'_>) -> Result<(), Error>) -> Result<(), Error> {
        match self {
            Self::Document(_, _) => {}
            Self::Node(v) => {
                if let Some(c) = v.children_arc() {
                    visit(Value::Nodes(c))?;
                }
            }
            Self::Nodes(v) => {
                for c in v.iter() {
                    visit(Value::Node(c))?;
                }
            }
            Self::Pages(v) => {
                for c in v.iter() {
                    visit(Value::Nodes(c))?;
                }
            }
            Self::Component(v) => visit(Value::Node(&v.root))?,
            Self::Components(v) => {
                for c in v.iter() {
                    visit(Value::Component(c))?;
                }
            }
            Self::Scoped(v) => {
                for root in v.retained_roots() {
                    visit(Value::Node(root))?;
                }
                visit(Value::Components(v.components().components_arc()))?;
            }
        }
        Ok(())
    }
    fn measure(self, limit: usize) -> Result<usize, CaptureSizeError> {
        let (fixed, dynamic) = match self {
            Self::Document(v, bytes) => (bytes, command_capture_value_bytes(v.as_ref(), limit)?),
            Self::Node(v) => (
                v.storage_bytes(),
                command_capture_node_bytes(v.metadata(), true, limit)?,
            ),
            Self::Component(v) => (
                v.storage_bytes(),
                command_capture_value_bytes(&(v.id.as_str(), v.name.as_str()), limit)?,
            ),
            Self::Nodes(v) => (edge_bytes(v), 0),
            Self::Pages(v) => (edge_bytes(v), 0),
            Self::Components(v) => (edge_bytes(v), 0),
            Self::Scoped(v) => (v.storage_bytes(), 0),
        };
        fixed
            .checked_add(dynamic)
            .ok_or(CaptureSizeError::Capacity {
                attempted: usize::MAX,
                limit,
            })
    }
}
fn edge_bytes<T>(v: &Arc<Vec<T>>) -> usize {
    size_of::<Vec<T>>() + 2 * size_of::<usize>() + v.capacity() * size_of::<T>()
}
fn add(a: &mut usize, b: usize) -> Result<(), Error> {
    *a = a
        .checked_add(b)
        .ok_or(Error::Charge(CaptureSizeError::Capacity {
            attempted: usize::MAX,
            limit: usize::MAX / 2,
        }))?;
    Ok(())
}
impl HistoryGraphCache {
    pub(super) fn clear(&mut self) {
        let (max_bytes, max_entries) = (self.max_bytes, self.max_entries);
        *self = Self::default();
        self.max_bytes = max_bytes;
        self.max_entries = max_entries;
    }
    pub(super) fn retained_bytes(&self) -> usize {
        // Twice capacity covers hash-table spare buckets, control bytes and
        // allocator rounding. Outgoing-key arrays use exact requested capacity.
        self.entries
            .capacity()
            .saturating_mul(2 * size_of::<(Key, Entry)>() + 32)
            .saturating_add(
                self.roots
                    .capacity()
                    .saturating_mul(2 * size_of::<Key>() + 32),
            )
            .saturating_add(self.child_bytes)
    }
    fn reserve(&mut self, children: usize) -> Result<(), Error> {
        if self.entries.len() >= self.max_entries {
            return Err(Error::Capacity);
        }
        let capacity = if self.entries.len() == self.entries.capacity() {
            self.entries.capacity().saturating_mul(2).saturating_add(4)
        } else {
            self.entries.capacity()
        };
        let bytes = capacity
            .saturating_mul(2 * size_of::<(Key, Entry)>() + 32)
            .saturating_add(
                self.roots
                    .capacity()
                    .saturating_mul(2 * size_of::<Key>() + 32),
            )
            .saturating_add(self.child_bytes)
            .saturating_add(children.saturating_mul(size_of::<Key>()));
        if bytes > self.max_bytes {
            return Err(Error::Capacity);
        }
        self.entries.reserve(1);
        Ok(())
    }
    fn attach(&mut self, value: Value<'_>, limit: usize) -> Result<(), Error> {
        let key = value.key();
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.references = entry.references.checked_add(1).ok_or(Error::Capacity)?;
            return Ok(());
        }
        self.reserve(value.child_count())?;
        let charge = value.measure(limit)?;
        let mut children = Vec::with_capacity(value.child_count());
        self.child_bytes = self
            .child_bytes
            .saturating_add(children.capacity() * size_of::<Key>());
        value.visit(|child| {
            children.push(child.key());
            self.attach(child, limit)
        })?;
        self.reserve(0)?;
        add(&mut self.total, charge)?;
        self.entries.insert(
            key,
            Entry {
                _owner: value.owner(),
                references: 1,
                charge,
                children,
            },
        );
        self.additions += 1;
        Ok(())
    }
    fn detach(&mut self, key: Key) {
        let entry = self
            .entries
            .get_mut(&key)
            .expect("retained immutable graph edge");
        entry.references -= 1;
        if entry.references != 0 {
            return;
        }
        let entry = self.entries.remove(&key).unwrap();
        self.total -= entry.charge;
        self.child_bytes -= entry.children.capacity() * size_of::<Key>();
        for child in entry.children {
            self.detach(child);
        }
    }
    fn update(&mut self, state: &History, limit: usize) -> Result<usize, Error> {
        self.additions = 0;
        let mut next = HashMap::new();
        let mut backing = HashSet::new();
        let mut workspaces = HashSet::new();
        let mut extra = 0usize;
        for entry in state.past.iter().chain(state.future.iter()) {
            extra = extra.checked_add(size_of_val(entry)).ok_or(Error::Charge(
                CaptureSizeError::Capacity {
                    attempted: usize::MAX,
                    limit,
                },
            ))?;
            let (values, components) = if let Some(snapshot) = entry.full_snapshot() {
                extra = extra.saturating_add(snapshot.command_clone_bytes());
                if snapshot
                    .command_workspace_identity()
                    .is_some_and(|owner| workspaces.insert(owner))
                {
                    if let Some((_, bytes)) = snapshot.command_workspace_retention(limit)? {
                        extra = extra.saturating_add(bytes);
                    }
                }
                (
                    [
                        Some(Value::Document(
                            snapshot.doc.metadata_arc(),
                            snapshot.doc.metadata_storage_bytes(),
                        )),
                        Some(Value::Nodes(snapshot.doc.root_children_arc())),
                        Some(Value::Pages(snapshot.doc.page_children_arcs())),
                        Some(Value::Components(snapshot.components.components_arc())),
                    ],
                    &snapshot.components,
                )
            } else {
                let snapshot = entry
                    .scoped_snapshot()
                    .expect("full or scoped history entry");
                (
                    [Some(Value::Scoped(snapshot)), None, None, None],
                    snapshot.components(),
                )
            };
            for value in values.into_iter().flatten() {
                if !next.contains_key(&value.key()) && next.len() >= self.max_entries {
                    return Err(Error::Capacity);
                }
                next.entry(value.key()).or_insert(value);
                if next.capacity().saturating_mul(2 * size_of::<Key>() + 32) > self.max_bytes / 8 {
                    return Err(Error::Capacity);
                }
            }
            if backing.insert(components.backing_identity()) {
                extra = extra
                    .checked_add(components.backing_storage_bytes())
                    .ok_or(Error::Charge(CaptureSizeError::Capacity {
                        attempted: usize::MAX,
                        limit,
                    }))?;
            }
        }
        // Root descriptions are bounded too; a pathological page/checkpoint
        // count falls back before creating a persistent ownership index.
        if next.len() > self.max_entries
            || next.capacity().saturating_mul(2 * size_of::<Key>() + 32) > self.max_bytes / 8
        {
            return Err(Error::Capacity);
        }
        let removed: Vec<_> = self
            .roots
            .iter()
            .filter(|k| !next.contains_key(k))
            .copied()
            .collect();
        for key in removed {
            self.roots.remove(&key);
            self.detach(key);
        }
        for (key, value) in next {
            if self.roots.insert(key) {
                self.attach(value, limit)?;
            }
        }
        if self.entries.is_empty() {
            self.entries.shrink_to_fit();
            self.roots.shrink_to_fit();
        } else if self.entries.len() < self.entries.capacity() / 4 {
            self.entries.shrink_to_fit();
        }
        if self.retained_bytes() > self.max_bytes {
            return Err(Error::Capacity);
        }
        self.total
            .checked_add(extra)
            .ok_or(Error::Charge(CaptureSizeError::Capacity {
                attempted: usize::MAX,
                limit,
            }))
    }
    pub(super) fn measure(
        &mut self,
        state: &History,
        limit: usize,
    ) -> Result<Option<usize>, CaptureSizeError> {
        match self.update(state, limit) {
            Ok(charge) => Ok(Some(charge)),
            Err(Error::Capacity) => {
                self.clear();
                Ok(None)
            }
            Err(Error::Charge(e)) => {
                self.clear();
                Err(e)
            }
        }
    }
}
