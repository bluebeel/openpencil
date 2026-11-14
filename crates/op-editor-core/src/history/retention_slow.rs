//! One identity-aware bound for immutable history retained by a cohort.
use crate::history_snapshot::{
    command_capture_node_bytes, command_capture_value_bytes, CaptureSizeError,
};
use crate::{history::History, HistoryEntry, SharedComponents, SharedNode};
use std::{collections::HashSet, sync::Arc};

struct Ledger {
    bytes: usize,
    limit: usize,
    nodes: HashSet<usize>,
    edges: HashSet<usize>,
    metadata: HashSet<usize>,
    components: HashSet<usize>,
    backing: HashSet<usize>,
    scoped: HashSet<usize>,
    workspaces: HashSet<usize>,
}
impl Ledger {
    fn add(&mut self, bytes: usize) -> Result<(), CaptureSizeError> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > self.limit {
            Err(CaptureSizeError::Capacity {
                attempted: self.bytes,
                limit: self.limit,
            })
        } else {
            Ok(())
        }
    }
    fn node(&mut self, root: &Arc<SharedNode>) -> Result<(), CaptureSizeError> {
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            if !self.nodes.insert(Arc::as_ptr(node) as usize) {
                continue;
            }
            self.add(node.storage_bytes())?;
            self.add(command_capture_node_bytes(
                node.metadata(),
                true,
                self.limit.saturating_sub(self.bytes),
            )?)?;
            if let Some((owner, bytes)) = node.child_edge_storage() {
                if !self.edges.insert(owner) {
                    continue;
                }
                self.add(bytes)?;
            }
            if let Some(children) = node.children() {
                self.add(children.len() * std::mem::size_of::<usize>() * 4)?;
                pending.extend(children);
            }
        }
        Ok(())
    }
    fn components(&mut self, components: &SharedComponents) -> Result<(), CaptureSizeError> {
        let (owner, bytes) = components.edge_storage();
        if self.edges.insert(owner) {
            self.add(bytes)?;
            for component in components.components() {
                if self.components.insert(Arc::as_ptr(component) as usize) {
                    self.add(
                        component.storage_bytes()
                            + 8 * (component.id.as_str().len() + component.name.len()),
                    )?;
                    self.node(&component.root)?;
                }
            }
        }
        if self.backing.insert(components.backing_identity()) {
            self.add(components.backing_storage_bytes())?;
        }
        Ok(())
    }
}
pub(super) fn measure(state: &History, limit: usize) -> Result<usize, CaptureSizeError> {
    let mut ledger = Ledger {
        bytes: 0,
        limit,
        nodes: HashSet::new(),
        edges: HashSet::new(),
        metadata: HashSet::new(),
        components: HashSet::new(),
        backing: HashSet::new(),
        scoped: HashSet::new(),
        workspaces: HashSet::new(),
    };
    for entry in state.past.iter().chain(&state.future) {
        match entry {
            HistoryEntry::Full(snapshot) => {
                ledger.add(snapshot.command_clone_bytes())?;
                let metadata = snapshot.doc.metadata_arc();
                if ledger.metadata.insert(Arc::as_ptr(metadata) as usize) {
                    ledger.add(snapshot.doc.metadata_storage_bytes())?;
                    ledger.add(command_capture_value_bytes(
                        snapshot.doc.document_metadata(),
                        limit.saturating_sub(ledger.bytes),
                    )?)?;
                }
                for (owner, bytes) in snapshot.doc.edge_storage() {
                    if ledger.edges.insert(owner) {
                        ledger.add(bytes)?;
                    }
                }
                for root in snapshot.doc.root_children() {
                    ledger.node(root)?;
                }
                for page in snapshot.doc.page_children_arcs().iter() {
                    for root in page.iter() {
                        ledger.node(root)?;
                    }
                }
                ledger.components(&snapshot.components)?;
                if snapshot
                    .command_workspace_identity()
                    .is_some_and(|owner| ledger.workspaces.insert(owner))
                {
                    if let Some((_, bytes)) =
                        snapshot.command_workspace_retention(limit.saturating_sub(ledger.bytes))?
                    {
                        ledger.add(bytes)?;
                    }
                }
            }
            HistoryEntry::Scoped(snapshot) => {
                if ledger.scoped.insert(Arc::as_ptr(snapshot) as usize) {
                    ledger.add(snapshot.storage_bytes())?;
                }
                for root in snapshot.retained_roots() {
                    ledger.node(root)?;
                }
                ledger.components(snapshot.components())?;
            }
        }
    }
    Ok(ledger.bytes)
}
