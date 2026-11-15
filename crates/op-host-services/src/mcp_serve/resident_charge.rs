//! Conservative resident-state accounting; no document-sized JSON buffers.
//!
//! This is an admission ledger, not an allocator/RSS measurement. JSON bytes
//! account for strings/embedded images; node storage and fixed runtime state
//! are charged separately. Shared history subtrees are visited once per Arc.

use std::{collections::HashSet, io::Write, sync::Arc};

use jian_ops_schema::node::PenNode;
use op_editor_core::{EditorState, PenDocument, PenNodeExt, SharedComponent, SharedNode};
use serde::Serialize;

#[path = "charge_cache.rs"]
mod charge_cache;
#[path = "charge_formatter.rs"]
mod charge_formatter;
use charge_cache::{RootCache, RootCharge};
#[path = "resident_charge_delta.rs"]
mod delta;
#[path = "history_charge.rs"]
mod history_charge;
pub(super) use delta::DocumentDelta;

#[derive(Default)]
pub(super) struct ChargeCache {
    document: Option<delta::DocumentCharge>,
    nodes: RootCache<SharedNode>,
    documents: RootCache<PenDocument>,
    components: RootCache<SharedComponent>,
    history: history_charge::HistoryChargeCache,
}

const SERIALIZED_MULTIPLIER: usize = 4;
const RUNTIME_ALLOWANCE: usize = 64 * 1024;

#[derive(Debug)]
pub(super) enum ChargeError {
    OverLimit,
    Serialization,
}

struct Counter {
    bytes: usize,
    limit: usize,
    exceeded: bool,
}

impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.saturating_add(bytes.len());
        if self.bytes > self.limit {
            self.exceeded = true;
            return Err(std::io::ErrorKind::OutOfMemory.into());
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Ledger {
    serialized: Counter,
    nodes: usize,
    limit: usize,
    cache_bytes: usize,
    shared_storage: usize,
}

impl Ledger {
    fn serialize(&mut self, value: &impl Serialize) -> Result<(), ChargeError> {
        value
            .serialize(&mut serde_json::Serializer::with_formatter(
                &mut self.serialized,
                charge_formatter::ChargeFormatter,
            ))
            .map_err(|_| {
                if self.serialized.exceeded {
                    ChargeError::OverLimit
                } else {
                    ChargeError::Serialization
                }
            })
    }

    fn node_storage(&mut self, nodes: &[PenNode], capacity: usize) -> Result<(), ChargeError> {
        // Retained empty capacity still owns PenNode-sized allocation slots.
        self.nodes = self.nodes.saturating_add(capacity);
        for node in nodes {
            // Sparse optional groups no longer occupy inline PenNode slots.
            // Keep the same conservative storage reserve for their owned boxes;
            // nested strings/arrays remain covered by serialized accounting.
            self.shared_storage = self
                .shared_storage
                .saturating_add(node.boxed_optional_storage_bytes().saturating_mul(2));
            if let Some(children) = node.children() {
                self.node_storage(children, children.capacity())?;
            }
        }
        if self.total() > self.limit {
            return Err(ChargeError::OverLimit);
        }
        Ok(())
    }

    fn root(&mut self, root: &PenNode) -> Result<(), ChargeError> {
        self.serialize(root)?;
        self.node_storage(std::slice::from_ref(root), 1)
    }

    fn total(&self) -> usize {
        RUNTIME_ALLOWANCE
            .saturating_add(self.cache_bytes)
            .saturating_add(self.shared_storage)
            .saturating_add(self.serialized.bytes.saturating_mul(SERIALIZED_MULTIPLIER))
            .saturating_add(
                self.nodes
                    .saturating_mul(std::mem::size_of::<PenNode>())
                    .saturating_mul(2),
            )
    }

    fn add_root(&mut self, charge: RootCharge) -> Result<(), ChargeError> {
        self.serialized.bytes = self.serialized.bytes.saturating_add(charge.serialized);
        self.nodes = self.nodes.saturating_add(charge.nodes);
        self.shared_storage = self.shared_storage.saturating_add(charge.shared_bytes);
        if self.total() > self.limit {
            Err(ChargeError::OverLimit)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
pub(super) fn resident_charge(state: &EditorState, limit: usize) -> Result<usize, ChargeError> {
    resident_charge_cached(state, limit, &mut ChargeCache::default())
}

fn new_ledger(limit: usize) -> Ledger {
    Ledger {
        serialized: Counter {
            bytes: 0,
            limit: limit / SERIALIZED_MULTIPLIER,
            exceeded: false,
        },
        nodes: 0,
        limit,
        cache_bytes: 0,
        shared_storage: 0,
    }
}

fn measure_shared(node: &SharedNode, limit: usize) -> Result<RootCharge, ChargeError> {
    let mut ledger = new_ledger(limit);
    ledger.serialize(node.metadata())?;
    Ok(RootCharge {
        serialized: ledger.serialized.bytes,
        nodes: 0,
        shared_bytes: node.storage_bytes(),
    })
}
fn charge_shared(
    root: &Arc<SharedNode>,
    ledger: &mut Ledger,
    cache: &mut RootCache<SharedNode>,
    visited: &mut HashSet<usize>,
    edges: &mut HashSet<usize>,
) -> Result<(), ChargeError> {
    if !visited.insert(Arc::as_ptr(root) as usize) {
        return Ok(());
    }
    let charge = match cache.get(root) {
        Some(charge) => charge,
        None => {
            let charge = measure_shared(root, ledger.limit)?;
            cache.insert(root, charge);
            charge
        }
    };
    ledger.add_root(charge)?;
    if let Some((identity, bytes)) = root.child_edge_storage() {
        if !edges.insert(identity) {
            return Ok(());
        }
        ledger.shared_storage = ledger.shared_storage.saturating_add(bytes);
    }
    if let Some(children) = root.children() {
        for child in children {
            charge_shared(child, ledger, cache, visited, edges)?
        }
    }
    Ok(())
}

fn charge_edge(
    (identity, bytes): (usize, usize),
    ledger: &mut Ledger,
    edges: &mut HashSet<usize>,
) -> bool {
    if edges.insert(identity) {
        ledger.shared_storage = ledger.shared_storage.saturating_add(bytes);
        true
    } else {
        false
    }
}

pub(super) fn resident_charge_cached(
    state: &EditorState,
    limit: usize,
    cache: &mut ChargeCache,
) -> Result<usize, ChargeError> {
    resident_charge_mode(state, limit, cache, true)
}

/// The caller holds the resident writer lock and observed every canonical
/// command through trusted captures. Ordinary/public mutation paths keep the
/// complete measurement API above; a revision alone never authorizes reuse.
pub(super) fn resident_charge_after_commands(
    state: &EditorState,
    limit: usize,
    cache: &mut ChargeCache,
    delta: Option<DocumentDelta>,
) -> Result<usize, ChargeError> {
    let document = cache
        .document
        .take()
        .and_then(|old| delta.and_then(|delta| old.adjust(state, delta)));
    resident_charge_with_document(state, limit, cache, true, document)
}

fn resident_charge_mode(
    state: &EditorState,
    limit: usize,
    cache: &mut ChargeCache,
    reuse_history: bool,
) -> Result<usize, ChargeError> {
    resident_charge_with_document(state, limit, cache, reuse_history, None)
}

fn resident_charge_with_document(
    state: &EditorState,
    limit: usize,
    cache: &mut ChargeCache,
    reuse_history: bool,
    document: Option<RootCharge>,
) -> Result<usize, ChargeError> {
    cache.nodes.prune();
    cache.components.prune();
    cache.documents.prune();
    let mut ledger = new_ledger(limit);
    ledger.shared_storage = ledger
        .shared_storage
        .saturating_add(state.history.runtime_storage_bytes());
    let document = match document {
        Some(document) => document,
        None => {
            let mut count = new_ledger(limit);
            count.serialize(&state.doc)?;
            count.node_storage(&state.doc.children, state.doc.children.capacity())?;
            if let Some(pages) = &state.doc.pages {
                for page in pages {
                    count.node_storage(&page.children, page.children.capacity())?;
                }
            }
            RootCharge {
                serialized: count.serialized.bytes,
                nodes: count.nodes,
                shared_bytes: count.shared_storage,
            }
        }
    };
    ledger.add_root(document)?;
    cache.document = Some(delta::DocumentCharge::new(state, document));
    ledger.serialize(&state.clipboard)?;
    ledger.node_storage(&state.clipboard, state.clipboard.capacity())?;
    for component in &state.components.components {
        ledger.root(&component.root)?;
    }
    if let Some(charge) = if reuse_history {
        cache.history.measure(state, limit)?
    } else {
        None
    } {
        let bytes = cache.history.retained_bytes();
        let added = charge
            .serialized
            .saturating_mul(SERIALIZED_MULTIPLIER)
            .saturating_add(charge.shared_bytes)
            .saturating_add(
                charge
                    .nodes
                    .saturating_mul(std::mem::size_of::<PenNode>() * 2),
            )
            .saturating_add(bytes);
        if ledger.total().saturating_add(added) <= limit {
            cache.nodes = RootCache::default();
            cache.documents = RootCache::default();
            cache.components = RootCache::default();
            ledger.add_root(charge)?;
            ledger.cache_bytes = bytes;
            return Ok(ledger.total());
        }
        // The optimization must never reject an authority that fits after
        // discarding cache storage. The uncached bounded ledger is the fallback.
        cache.history.clear();
    }
    let mut nodes = HashSet::new();
    let mut components = HashSet::new();
    let mut documents = HashSet::new();
    let mut backing = HashSet::new();
    let mut edges = HashSet::new();
    let mut scoped = HashSet::new();
    for entry in state.history.past.iter().chain(state.history.future.iter()) {
        ledger.shared_storage = ledger
            .shared_storage
            .saturating_add(std::mem::size_of_val(entry));
        let snapshot_components = if let Some(snapshot) = entry.full_snapshot() {
            let mut doc_edges = snapshot.doc.edge_storage();
            let roots_new = charge_edge(doc_edges.next().unwrap(), &mut ledger, &mut edges);
            let pages_new = charge_edge(doc_edges.next().unwrap(), &mut ledger, &mut edges);
            let metadata = snapshot.doc.metadata_arc();
            if documents.insert(Arc::as_ptr(metadata) as usize) {
                let charge = match cache.documents.get(metadata) {
                    Some(charge) => charge,
                    None => {
                        let mut count = new_ledger(ledger.limit);
                        count.serialize(metadata.as_ref())?;
                        let bytes = snapshot.doc.metadata_storage_bytes();
                        let charge = RootCharge {
                            serialized: count.serialized.bytes,
                            nodes: 0,
                            shared_bytes: bytes,
                        };
                        cache.documents.insert(metadata, charge);
                        charge
                    }
                };
                ledger.add_root(charge)?;
            }
            if roots_new {
                for root in snapshot.doc.root_children() {
                    charge_shared(root, &mut ledger, &mut cache.nodes, &mut nodes, &mut edges)?;
                }
            }
            if pages_new {
                for (i, storage) in doc_edges.enumerate() {
                    if charge_edge(storage, &mut ledger, &mut edges) {
                        for root in snapshot.doc.page_children(i).unwrap() {
                            charge_shared(
                                root,
                                &mut ledger,
                                &mut cache.nodes,
                                &mut nodes,
                                &mut edges,
                            )?;
                        }
                    }
                }
            }
            &snapshot.components
        } else {
            let snapshot = entry
                .scoped_snapshot()
                .expect("full or scoped history entry");
            if scoped.insert(Arc::as_ptr(snapshot) as usize) {
                ledger.shared_storage = ledger
                    .shared_storage
                    .saturating_add(snapshot.storage_bytes());
            }
            for root in snapshot.retained_roots() {
                charge_shared(root, &mut ledger, &mut cache.nodes, &mut nodes, &mut edges)?;
            }
            snapshot.components()
        };
        let components_new =
            charge_edge(snapshot_components.edge_storage(), &mut ledger, &mut edges);
        if backing.insert(snapshot_components.backing_identity()) {
            ledger.shared_storage = ledger
                .shared_storage
                .saturating_add(snapshot_components.backing_storage_bytes());
        }
        if components_new {
            for component in snapshot_components.components() {
                if components.insert(Arc::as_ptr(component) as usize) {
                    let charge = match cache.components.get(component) {
                        Some(charge) => charge,
                        None => {
                            let mut count = new_ledger(ledger.limit);
                            count.serialize(&(component.id.as_str(), component.name.as_str()))?;
                            let charge = RootCharge {
                                serialized: count.serialized.bytes,
                                nodes: 0,
                                shared_bytes: component.storage_bytes(),
                            };
                            cache.components.insert(component, charge);
                            charge
                        }
                    };
                    ledger.add_root(charge)?;
                }
                charge_shared(
                    &component.root,
                    &mut ledger,
                    &mut cache.nodes,
                    &mut nodes,
                    &mut edges,
                )?;
            }
        }
    }
    // A background save can hold roots no longer present in history. The
    // cache must not keep their weak slots after dropping the final owner.
    cache.nodes.retain_visited(&nodes);
    cache.components.retain_visited(&components);
    cache.documents.retain_visited(&documents);
    ledger.cache_bytes = cache
        .nodes
        .retained_bytes()
        .saturating_add(cache.components.retained_bytes())
        .saturating_add(cache.documents.retained_bytes());
    let charge = ledger.total();
    if charge > limit {
        Err(ChargeError::OverLimit)
    } else {
        Ok(charge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_empty_node_buffers_are_charged_until_released() {
        let mut state = EditorState::new();
        assert!(resident_charge(&state, 128 * 1024).is_ok());
        state.doc.children.reserve(2000);
        assert!(state.doc.children.is_empty());
        assert!(matches!(
            resident_charge(&state, 128 * 1024),
            Err(ChargeError::OverLimit)
        ));
        state.doc.children.shrink_to_fit();
        assert!(resident_charge(&state, 128 * 1024).is_ok());
    }

    #[test]
    fn cached_history_measurements_follow_replacement_and_enforce_smaller_limits() {
        let document = jian_ops_schema::load_str(r#"{"version":"1.0.0","children":[{"type":"rectangle","id":"r","width":20,"height":30}]}"#).unwrap().value;
        let mut state = EditorState::from_document(document);
        state.history_push_past(state.snapshot_for_history());
        state.history_push_past(state.snapshot_for_history());
        let mut cache = ChargeCache::default();
        let first = resident_charge_cached(&state, 16 * 1024 * 1024, &mut cache).unwrap();
        assert_eq!(
            first,
            resident_charge_cached(&state, 16 * 1024 * 1024, &mut cache).unwrap()
        );
        let mut fallback = ChargeCache::default();
        let minimum = resident_charge_mode(&state, 16 * 1024 * 1024, &mut fallback, false).unwrap();
        assert!(matches!(
            resident_charge_cached(&state, minimum - 1, &mut cache),
            Err(ChargeError::OverLimit)
        ));
        let mut replacement = state.doc.clone();
        replacement.children[0].base_mut().name = Some("larger historical node".repeat(100));
        state
            .history
            .past
            .back_mut()
            .unwrap()
            .full_snapshot_mut()
            .unwrap()
            .doc = op_editor_core::SharedDoc::capture(&replacement, None);
        let changed = resident_charge_cached(&state, 16 * 1024 * 1024, &mut cache).unwrap();
        assert!(changed > first);
        assert_eq!(changed, resident_charge(&state, 16 * 1024 * 1024).unwrap());
        state.history.past.clear();
        let cleared = resident_charge_cached(&state, 16 * 1024 * 1024, &mut cache).unwrap();
        assert_eq!(cleared, resident_charge(&state, 16 * 1024 * 1024).unwrap());
        assert_eq!(cache.nodes.retained_bytes(), 0);
    }
}

#[cfg(test)]
#[path = "resident_charge_recursive_tests.rs"]
mod recursive_tests;

#[cfg(test)]
#[path = "history_charge_tests.rs"]
mod history_charge_tests;

#[cfg(test)]
#[path = "resident_charge_scoped_tests.rs"]
mod scoped_tests;

#[cfg(test)]
#[path = "resident_charge_boxed_tests.rs"]
mod boxed_tests;
