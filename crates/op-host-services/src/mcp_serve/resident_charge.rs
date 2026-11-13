//! Conservative resident-state accounting; no document-sized JSON buffers.
//!
//! This is an admission ledger, not an allocator/RSS measurement. JSON bytes
//! account for strings/embedded images; node storage and fixed runtime state
//! are charged separately. Shared history subtrees are visited once per Arc.

use std::{collections::HashSet, io::Write, sync::Arc};

use jian_ops_schema::node::PenNode;
use op_editor_core::{EditorState, PenNodeExt};
use serde::Serialize;

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
}

impl Ledger {
    fn serialize(&mut self, value: &impl Serialize) -> Result<(), ChargeError> {
        serde_json::to_writer(&mut self.serialized, value).map_err(|_| {
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
            .saturating_add(self.serialized.bytes.saturating_mul(SERIALIZED_MULTIPLIER))
            .saturating_add(
                self.nodes
                    .saturating_mul(std::mem::size_of::<PenNode>())
                    .saturating_mul(2),
            )
    }
}

pub(super) fn resident_charge(state: &EditorState, limit: usize) -> Result<usize, ChargeError> {
    let mut ledger = Ledger {
        serialized: Counter {
            bytes: 0,
            limit: limit / SERIALIZED_MULTIPLIER,
            exceeded: false,
        },
        nodes: 0,
        limit,
    };
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
            if nodes.insert(Arc::as_ptr(root)) {
                ledger.root(root)?;
            }
        }
        for component in snapshot.components.components() {
            if components.insert(Arc::as_ptr(component)) {
                ledger.root(&component.root)?;
            }
        }
    }
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
}
