//! Exact accepted Move/Reorder receipts for one exclusive command authority.
//! Only the changed sibling arrays are verified; subtree tokens move intact.
use super::{at, children_at, Cache, Edit, Index, Pending, Scope, C};
use crate::{walkers::ReorderDirection, EditorState, NodeId, PenNodeExt};

// The real Batch applier observes each leaf through apply_with_allocator.
// Scalar fusion cannot change locations. Nested/unknown structural programs
// keep the existing invalidation, and Batch rollback invalidates the authority.
pub(super) fn observed_batch(command: &C) -> bool {
    let C::Batch { commands } = command else {
        return false;
    };
    commands.len() <= 4096
        && commands.iter().all(|command| {
            !matches!(command, C::Batch { .. })
                && (super::preserves_locations(command)
                    || super::insertion::supports(command)
                    || super::wrapping::supports(command)
                    || super::cloning::supports(command)
                    || matches!(command, C::MoveNode { .. } | C::ReorderSelected { .. }))
        })
}
#[derive(Debug)]
pub(super) struct Motion {
    source: usize,
    from_parent: usize,
    to_parent: usize,
    from: usize,
    to: usize,
    from_len: usize,
    to_len: usize,
}
impl Index {
    pub(super) fn unique(&self, root: usize, id: &str) -> Result<Option<usize>, ()> {
        let mut found = None;
        for &key in self.ids.get(id).map_or(&[][..], |keys| keys.keys()) {
            if self.entries[key].root == root {
                if found.replace(key).is_some() {
                    return Err(());
                }
            }
        }
        Ok(found)
    }
}
impl Motion {
    pub(super) fn pending(index: &Index, state: &EditorState, command: &C) -> Option<Pending> {
        let (scope, source, parent, requested) = match command {
            C::MoveNode {
                node_id,
                target_parent,
                page_id,
                index: requested,
            } => {
                if !node_id.is_real() || node_id == target_parent {
                    return None;
                }
                let scope =
                    state.command_lookup_scope(page_id.as_deref(), state.ui.active_page_index)?;
                let root = index.root(&scope)?;
                let source = index.unique(root, node_id.as_str()).ok()??;
                let parent = if target_parent.is_real() {
                    index.unique(root, target_parent.as_str()).ok()??
                } else {
                    root
                };
                if index.path(parent).starts_with(&index.path(source)) {
                    return None;
                }
                if parent != root && !at(&state.doc, &scope, &index.path(parent))?.is_container() {
                    return None;
                }
                (scope, source, parent, *requested)
            }
            C::ReorderSelected { direction } => {
                let scope = state.command_lookup_active_scope();
                let root = index.root(&scope)?;
                let source = index.unique(root, state.selection.anchor.as_str()).ok()??;
                let row = &index.entries[source];
                let parent = row.parent?;
                let count = index.entries[parent].children.len();
                // A boundary reorder is rejected without mutation. Keeping an
                // unchanged receipt avoids throwing away its exact index.
                let requested = match direction {
                    ReorderDirection::Up => row.slot.saturating_sub(1),
                    ReorderDirection::Down => row.slot.saturating_add(1).min(count - 1),
                };
                (scope, source, parent, Some(requested))
            }
            _ => return None,
        };
        let from_parent = index.entries[source].parent?;
        let from_len = index.entries[from_parent].children.len();
        let to_len = index.entries[parent].children.len();
        let limit = to_len.checked_sub(usize::from(from_parent == parent))?;
        let motion = Self {
            source,
            from_parent,
            to_parent: parent,
            from: index.entries[source].slot,
            to: requested.unwrap_or(limit).min(limit),
            from_len,
            to_len,
        };
        Some(Pending {
            scope,
            parent: from_parent,
            len: from_len,
            revision: state.document_revision(),
            command: Some(Box::new(command.clone())),
            edit: Edit::Motion(motion),
        })
    }
    pub(super) fn before(&self, index: &Index, state: &EditorState, scope: &Scope) -> bool {
        let source = children_at(&state.doc, scope, &index.path(self.from_parent));
        let target = children_at(&state.doc, scope, &index.path(self.to_parent));
        source.is_some_and(|nodes| {
            nodes.len() == self.from_len
                && nodes
                    .get(self.from)
                    .is_some_and(|node| node.id_str() == index.entries[self.source].id.as_ref())
        }) && target.is_some_and(|nodes| nodes.len() == self.to_len)
    }
    fn destination(&self, index: &Index) -> Vec<usize> {
        let mut path = index.path(self.to_parent).into_vec();
        self.remove_path(&index.path(self.from_parent), &mut path);
        path
    }
    fn remove_path(&self, source_parent: &[usize], path: &mut [usize]) {
        if path.len() > source_parent.len()
            && path.starts_with(source_parent)
            && path[source_parent.len()] > self.from
        {
            path[source_parent.len()] -= 1;
        }
    }
    fn after_parent(&self, index: &Index, parent: usize) -> Vec<usize> {
        let destination = self.destination(index);
        let mut path = index.path(parent).into_vec();
        self.remove_path(&index.path(self.from_parent), &mut path);
        if path.len() > destination.len()
            && path.starts_with(&destination)
            && path[destination.len()] >= self.to
        {
            path[destination.len()] += 1;
        }
        path
    }
    fn expected(&self, index: &Index, parent: usize, slot: usize) -> usize {
        if parent == self.to_parent && slot == self.to {
            return self.source;
        }
        let mut old = slot - usize::from(parent == self.to_parent && slot > self.to);
        if parent == self.from_parent && old >= self.from {
            old += 1;
        }
        index.entries[parent].children[old]
    }
    fn matches(&self, index: &Index, state: &EditorState, scope: &Scope, parent: usize) -> bool {
        let length = index.entries[parent].children.len() - usize::from(parent == self.from_parent)
            + usize::from(parent == self.to_parent);
        children_at(&state.doc, scope, &self.after_parent(index, parent)).is_some_and(|nodes| {
            nodes.len() == length
                && nodes.iter().enumerate().all(|(slot, node)| {
                    node.id_str()
                        == index.entries[self.expected(index, parent, slot)]
                            .id
                            .as_ref()
                })
        })
    }
    pub(super) fn finish(
        &self,
        index: &mut Index,
        state: &EditorState,
        scope: &Scope,
    ) -> Option<()> {
        if !self.matches(index, state, scope, self.from_parent)
            || self.to_parent != self.from_parent
                && !self.matches(index, state, scope, self.to_parent)
        {
            return None;
        }
        let key = index.entries[self.from_parent].children.remove(self.from);
        index.entries[self.to_parent].children.insert(self.to, key);
        index.entries[key].parent = Some(self.to_parent);
        for parent in [self.from_parent, self.to_parent] {
            for slot in 0..index.entries[parent].children.len() {
                let key = index.entries[parent].children[slot];
                index.entries[key].slot = slot;
            }
        }
        Some(())
    }
}
impl Cache {
    pub(super) fn unique_path(
        &mut self,
        state: &EditorState,
        scope: &Scope,
        id: &NodeId,
    ) -> Option<Result<Option<Box<[usize]>>, ()>> {
        if !self.ready(state) {
            return None;
        }
        let index = match self {
            Self::Ready(index) | Self::Pending(index, _) => index,
            _ => return None,
        };
        Some(
            index
                .unique(index.root(scope)?, id.as_str())
                .map(|key| key.map(|key| index.path(key))),
        )
    }
}
impl EditorState {
    /// Only an entered authority may use this. Ambiguous IDs retain each
    /// operation's historical DFS/sibling-first resolver, never a guessed path.
    pub(crate) fn command_lookup_unique(
        &self,
        scope: &Scope,
        id: &NodeId,
    ) -> Option<Result<Option<Box<[usize]>>, ()>> {
        let seed = &self.command_id_seed;
        if !seed.entered || !seed.valid || seed.generation != self.document_generation() {
            return None;
        }
        seed.lookup.lock().ok()?.unique_path(self, scope, id)
    }
    pub(crate) fn command_reorder_at(
        &mut self,
        scope: &Scope,
        path: &[usize],
        direction: ReorderDirection,
    ) -> bool {
        let Some((&at, parent)) = path.split_last() else {
            return false;
        };
        let Some(nodes) = super::list_mut(&mut self.doc, scope, parent, false) else {
            return false;
        };
        let to = match direction {
            ReorderDirection::Up => at.checked_sub(1),
            ReorderDirection::Down if at + 1 < nodes.len() => Some(at + 1),
            _ => None,
        };
        let Some(to) = to.filter(|_| at < nodes.len()) else {
            return false;
        };
        nodes.swap(at, to);
        true
    }
}
