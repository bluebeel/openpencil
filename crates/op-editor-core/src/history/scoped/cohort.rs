//! Borrowed restoration/receipt bounds for selectively exchanged history.
//! Only the touched live payload and immutable opposite side are visited;
//! unrelated forests are never copied merely to group an Undo/Redo save.
use super::{
    identity,
    patch::{self, Scope},
    ScopedEditorSnapshot,
};
use crate::{history_snapshot::command_capture_node_bytes, EditorState, PenNodeExt, SharedNode};
use jian_ops_schema::{node::PenNode, PenDocument};
use std::{mem::size_of, sync::Arc};

pub(super) struct Bound {
    bytes: usize,
    limit: usize,
}
impl Bound {
    pub(super) fn add(&mut self, bytes: usize) -> Option<()> {
        self.bytes = self.bytes.checked_add(bytes)?;
        (self.bytes <= self.limit).then_some(())
    }
    pub(super) fn live(&mut self, root: &PenNode, header_only: bool) -> Option<()> {
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            self.add(
                command_capture_node_bytes(node, true, self.limit.checked_sub(self.bytes)?).ok()?,
            )?;
            if !header_only {
                if let Some(children) = node.children() {
                    self.add(children.capacity().checked_mul(size_of::<PenNode>() + 64)?)?;
                    pending.extend(children);
                }
            }
        }
        Some(())
    }
    fn shared(&mut self, root: &Arc<SharedNode>) -> Option<()> {
        let mut pending = vec![root.as_ref()];
        while let Some(node) = pending.pop() {
            self.add(node.storage_bytes())?;
            self.add(
                command_capture_node_bytes(
                    node.metadata(),
                    true,
                    self.limit.checked_sub(self.bytes)?,
                )
                .ok()?,
            )?;
            if let Some(children) = node.children() {
                self.add(children.len().checked_mul(size_of::<PenNode>() + 128)?)?;
                pending.extend(children.iter().map(Arc::as_ref));
            }
        }
        Some(())
    }
    /// Full list bookkeeping is real preparation/Vec relocation, while the
    /// untouched descendants remain borrowed. Include identities and spare
    /// sibling capacity, not merely the number of edited roots.
    pub(super) fn parent<'a>(
        &mut self,
        doc: &'a PenDocument,
        scope: &Scope,
        id: Option<&str>,
        index: &identity::Index,
    ) -> Option<&'a [PenNode]> {
        let (nodes, capacity) = match id {
            Some(id) => {
                let path = index.path(scope, id).ok()?;
                let parent = patch::node(patch::roots(doc, scope).ok()?, &path)?;
                let nodes = parent.children();
                (
                    nodes.map_or(&[][..], Vec::as_slice),
                    nodes.map_or(0, Vec::capacity),
                )
            }
            None => {
                let nodes = match scope {
                    Scope::Root => &doc.children,
                    Scope::Page { index, id } => {
                        let page = doc.pages.as_ref()?.get(*index)?;
                        if page.id != *id {
                            return None;
                        }
                        &page.children
                    }
                };
                (nodes.as_slice(), nodes.capacity())
            }
        };
        self.add(capacity.checked_mul(size_of::<PenNode>() + 512)?)?;
        for node in nodes {
            self.add(128usize.checked_add(node.id_str().len().checked_mul(32)?)?)?;
        }
        Some(nodes)
    }
}
impl ScopedEditorSnapshot {
    pub(crate) fn cohort_exchange_bound(&self, state: &EditorState, limit: usize) -> Option<usize> {
        if !self.actor_safe || self.header.is_some() || !self.page_headers.is_empty() {
            return None;
        }
        self.validate_shape(state).ok()?;
        let mut bound = Bound { bytes: 0, limit };
        // Clone/rebase the one scoped entry, its inverse on abort, auxiliary
        // ownership, and the exact History checkpoint are independently charged.
        // Immutable roots are already in the cohort history-union ledger; their
        // materialized opposite side below is an additional live allocation.
        bound.add(
            self.storage_bytes()
                .checked_mul(4)?
                .checked_add(16 * 1024)?,
        )?;
        let mut wanted = identity::Wanted::new();
        for patch in &self.patches {
            patch.want_cohort_identities(&mut wanted);
        }
        let index = identity::Index::new(&state.doc, wanted).ok()?;
        for patch in &self.patches {
            patch.cohort_bound(&state.doc, &index, &mut bound)?;
        }
        for root in self.retained_roots() {
            bound.shared(root)?;
        }
        for patch in &self.auxiliary_patches {
            bound.add(patch.cohort_live_bound(state, limit.checked_sub(bound.bytes)?)?)?;
        }
        Some(bound.bytes)
    }
}
