//! Conservative resident-state accounting; no document-sized JSON buffers.
//!
//! This is an admission ledger, not an allocator/RSS measurement. JSON bytes
//! account for strings/embedded images; node storage and fixed runtime state
//! are charged separately. Shared history subtrees are visited once per Arc.

use std::{collections::HashSet, io::Write, sync::Arc};

use jian_ops_schema::node::PenNode;
use op_editor_core::{EditorState, PenNodeExt};
use serde::Serialize;

#[path = "charge_cache.rs"]
mod charge_cache;
#[path = "charge_formatter.rs"]
mod charge_formatter;
use charge_cache::{RootCache, RootCharge};

#[derive(Default)]
pub(super) struct ChargeCache {
    nodes: RootCache<PenNode>,
    components: RootCache<op_editor_core::components::Component>,
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
        if self.total() > self.limit {
            Err(ChargeError::OverLimit)
        } else {
            Ok(())
        }
    }
}

pub(super) fn resident_charge(state: &EditorState, limit: usize) -> Result<usize, ChargeError> {
    resident_charge_cached(state, limit, &mut ChargeCache::default())
}

fn ledger(limit: usize) -> Ledger {
    Ledger {
        serialized: Counter {
            bytes: 0,
            limit: limit / SERIALIZED_MULTIPLIER,
            exceeded: false,
        },
        nodes: 0,
        limit,
        cache_bytes: 0,
    }
}

fn measure_root(root: &PenNode, limit: usize) -> Result<RootCharge, ChargeError> {
    let mut ledger = ledger(limit);
    ledger.root(root)?;
    Ok(RootCharge {
        serialized: ledger.serialized.bytes,
        nodes: ledger.nodes,
    })
}

pub(super) fn resident_charge_cached(
    state: &EditorState,
    limit: usize,
    cache: &mut ChargeCache,
) -> Result<usize, ChargeError> {
    cache.nodes.prune();
    cache.components.prune();
    let mut ledger = ledger(limit);
    ledger.serialize(&state.doc)?;
    ledger.node_storage(&state.doc.children, state.doc.children.capacity())?;
    if let Some(pages) = &state.doc.pages {
        for page in pages {
            ledger.node_storage(&page.children, page.children.capacity())?;
        }
    }
    ledger.serialize(&state.clipboard)?;
    ledger.node_storage(&state.clipboard, state.clipboard.capacity())?;
    for component in &state.components.components {
        ledger.root(&component.root)?;
    }
    let mut nodes = HashSet::new();
    let mut components = HashSet::new();
    for snapshot in state.history.past.iter().chain(state.history.future.iter()) {
        ledger.serialize(snapshot.doc.document_metadata())?;
        for root in snapshot
            .doc
            .root_children()
            .iter()
            .chain((0..).map_while(|i| snapshot.doc.page_children(i)).flatten())
        {
            if nodes.insert(Arc::as_ptr(root) as usize) {
                let charge = match cache.nodes.get(root) {
                    Some(charge) => charge,
                    None => {
                        let charge = measure_root(root, limit)?;
                        cache.nodes.insert(root, charge);
                        charge
                    }
                };
                ledger.add_root(charge)?;
            }
        }
        for component in snapshot.components.components() {
            if components.insert(Arc::as_ptr(component) as usize) {
                let charge = match cache.components.get(component) {
                    Some(charge) => charge,
                    None => {
                        let charge = measure_root(&component.root, limit)?;
                        cache.components.insert(component, charge);
                        charge
                    }
                };
                ledger.add_root(charge)?;
            }
        }
    }
    // A background save can hold roots no longer present in history. The
    // cache must not keep their weak slots after dropping the final owner.
    cache.nodes.retain_visited(&nodes);
    cache.components.retain_visited(&components);
    ledger.cache_bytes = cache
        .nodes
        .retained_bytes()
        .saturating_add(cache.components.retained_bytes());
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
        assert!(matches!(
            resident_charge_cached(&state, first - 1, &mut cache),
            Err(ChargeError::OverLimit)
        ));
        let mut replacement = state.doc.clone();
        replacement.children[0].base_mut().name = Some("larger historical node".repeat(100));
        state.history.past.back_mut().unwrap().doc =
            op_editor_core::SharedDoc::capture(&replacement, None);
        let changed = resident_charge_cached(&state, 16 * 1024 * 1024, &mut cache).unwrap();
        assert!(changed > first);
        assert_eq!(changed, resident_charge(&state, 16 * 1024 * 1024).unwrap());
        state.history.past.clear();
        let cleared = resident_charge_cached(&state, 16 * 1024 * 1024, &mut cache).unwrap();
        assert_eq!(cleared, resident_charge(&state, 16 * 1024 * 1024).unwrap());
        assert_eq!(cache.nodes.retained_bytes(), 0);
    }
}
