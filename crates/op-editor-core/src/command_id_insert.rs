//! Accepted append/root-placeholder receipts. Payloads are read from the real
//! canonical result, never cloned from a rich command merely to keep a cache.
use super::{at, children_at, Edit, Index, Order, Pending, Scope, C};
use crate::{EditorState, NodeId, PenNodeExt};
use jian_ops_schema::node::PenNode;

#[derive(Debug, PartialEq, Eq)]
struct Signature {
    family: u8,
    parent: NodeId,
    page: Option<String>,
    count: usize,
    frame: bool,
}
impl Signature {
    fn new(command: &C) -> Option<Self> {
        let (family, parent, page, count, frame) = match command {
            C::InsertNode {
                target_parent,
                page_id,
                ..
            } => (0, target_parent.clone(), page_id, 1, false),
            C::BatchInsert { items, page_id } => (1, NodeId::NONE, page_id, items.len(), false),
            C::InsertSubtree {
                nodes,
                parent_id,
                page_id,
            } => (
                2,
                parent_id.clone(),
                page_id,
                nodes.len(),
                matches!(nodes.as_slice(), [PenNode::Frame(_)]),
            ),
            C::InsertAuthoredSubtree {
                nodes,
                parent_id,
                page_id,
            } => (
                3,
                parent_id.clone(),
                page_id,
                nodes.len(),
                matches!(nodes.as_slice(), [PenNode::Frame(_)]),
            ),
            C::InsertAuthoredSubtreePreservingRoots {
                nodes,
                parent_id,
                page_id,
            } => (4, parent_id.clone(), page_id, nodes.len(), false),
            _ => return None,
        };
        if count == 0
            || count > 4096
            || parent.as_str().len() > 16384
            || page.as_ref().is_some_and(|page| page.len() > 16384)
        {
            return None;
        }
        Some(Self {
            family,
            parent,
            page: page.clone(),
            count,
            frame,
        })
    }
}
pub(super) fn supports(command: &C) -> bool {
    Signature::new(command).is_some()
}
#[derive(Debug)]
pub(super) struct Insertion {
    signature: Signature,
    remove: Option<usize>,
}
impl Insertion {
    pub(super) fn pending(index: &Index, state: &EditorState, command: &C) -> Option<Pending> {
        let signature = Signature::new(command)?;
        let scope =
            state.command_lookup_scope(signature.page.as_deref(), state.ui.active_page_index)?;
        let root = index.root(&scope)?;
        // Actual leaf/rich writers select the first DFS parent; duplicate
        // parent identities must not silently switch to sibling-first.
        let parent = if signature.parent.is_real() {
            index.select(root, signature.parent.as_str(), Order::Preorder)?
        } else {
            root
        };
        let path = index.path(parent);
        if parent != root && !at(&state.doc, &scope, &path)?.is_container() {
            return None;
        }
        let nodes = children_at(&state.doc, &scope, &path)?;
        let remove = if parent == root && signature.frame {
            nodes
                .iter()
                .find(|node| {
                    matches!(node, PenNode::Frame(_)) && node.children().is_none_or(Vec::is_empty)
                })
                .and_then(|node| {
                    // Match the real replacement's NodeId normalization and
                    // first raw-root match, even with authored duplicate IDs.
                    let id = NodeId::new(node.id_str());
                    nodes.iter().position(|node| node.id_str() == id.as_str())
                })
        } else {
            None
        };
        Some(Pending {
            scope,
            parent,
            len: nodes.len(),
            revision: state.document_revision(),
            command: None,
            edit: Edit::Insertion(Self { signature, remove }),
        })
    }
    pub(super) fn matches(&self, command: &C) -> bool {
        Signature::new(command).is_some_and(|signature| signature == self.signature)
    }
    pub(super) fn finish(
        &self,
        pending: &Pending,
        index: &mut Index,
        state: &EditorState,
    ) -> Option<()> {
        let nodes = children_at(&state.doc, &pending.scope, &index.path(pending.parent))?;
        let retained = pending
            .len
            .checked_sub(usize::from(self.remove.is_some()))?;
        if nodes.len() != retained.checked_add(self.signature.count)? {
            return None;
        }
        for (slot, node) in nodes[..retained].iter().enumerate() {
            let old = slot + usize::from(self.remove.is_some_and(|removed| slot >= removed));
            let key = *index.entries[pending.parent].children.get(old)?;
            if node.id_str() != index.entries[key].id.as_ref() {
                return None;
            }
        }
        if let Some(remove) = self.remove {
            index.remove(pending.parent, remove);
        }
        index.append(pending.parent, &nodes[retained..]);
        Some(())
    }
}
impl Pending {
    pub(super) fn same_command(&self, state: &EditorState, command: &C) -> bool {
        // A same-command observation can occur after the host swaps actor UI
        // without a canonical write. Selection receipts are re-prepared from
        // that fresh actor, retaining the same immutable location index.
        if matches!(self.edit, Edit::Motion(_) | Edit::Wrap(_)) {
            return false;
        }
        let raw = match command {
            C::InsertNode { page_id, .. }
            | C::BatchInsert { page_id, .. }
            | C::InsertSubtree { page_id, .. }
            | C::InsertAuthoredSubtree { page_id, .. }
            | C::InsertAuthoredSubtreePreservingRoots { page_id, .. }
            | C::CopyNode { page_id, .. }
            | C::DeleteNode { page_id, .. } => page_id.as_deref(),
            _ => return false,
        };
        if state
            .command_lookup_scope(raw, state.ui.active_page_index)
            .as_ref()
            != Some(&self.scope)
        {
            return false;
        }
        match &self.edit {
            Edit::Insertion(receipt) => receipt.matches(command),
            _ => self.command.as_deref() == Some(command),
        }
    }
}
impl EditorState {
    /// Some(Err) is an authoritative invalid destination; None is the fresh
    /// compatibility path outside an entered location authority.
    pub(crate) fn command_insertion_path(
        &self,
        scope: &Scope,
        parent: &NodeId,
    ) -> Option<Result<Box<[usize]>, ()>> {
        let seed = &self.command_id_seed;
        if !seed.entered || !seed.valid || seed.generation != self.document_generation() {
            return None;
        }
        if !parent.is_real() {
            return Some(Ok(Box::new([])));
        }
        let path = seed
            .lookup
            .lock()
            .ok()?
            .path(self, scope, parent.as_str(), Order::Preorder)?;
        Some(
            path.filter(|path| at(&self.doc, scope, path).is_some_and(PenNodeExt::is_container))
                .ok_or(()),
        )
    }
    #[allow(clippy::result_large_err)]
    pub(crate) fn command_extend_at(
        &mut self,
        scope: &Scope,
        path: &[usize],
        nodes: Vec<PenNode>,
    ) -> Result<(), Vec<PenNode>> {
        let capture = crate::node_creation_receipt::active(self);
        let count = nodes.len();
        let Some(destination) = super::list_mut(&mut self.doc, scope, path, true) else {
            return Err(nodes);
        };
        let slot = destination.len();
        destination.extend(nodes);
        if capture {
            let written = (count == 1)
                .then(|| crate::node_creation_receipt::written_id(&destination[slot]))
                .flatten();
            crate::node_creation_receipt::record_append(self, scope, path, slot, written);
        }
        Ok(())
    }
}
