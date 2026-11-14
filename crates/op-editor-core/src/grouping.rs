//! Group / ungroup mutators (`Cmd+G` / `Cmd+Shift+G` parity).
//!
//! Both ops resolve siblings under one parent on the active page. A group
//! inserts where its first member sat; ungroup splices its children back into
//! that same sibling array. Child records are moved without geometry changes.

use crate::id_allocator::{IdAllocError, IdAllocator, SequentialIdAllocator};
use crate::node_id::NodeId;
use crate::pen_node_ext::{make_group, PenNodeExt};
use crate::state::EditorState;
use jian_ops_schema::node::PenNode;

impl EditorState {
    /// Wrap the current selection in a fresh `Group` node inserted
    /// at the position of the first selected sibling. Supports root or
    /// nested same-parent selections. Returns the new group's
    /// id, or `None` when the selection is empty / spans parents.
    pub fn group_selected(&mut self, next_id: &mut u64) -> Option<NodeId> {
        let mut allocator = SequentialIdAllocator::for_document(&self.doc, *next_id).ok()?;
        let result = self
            .group_selected_with_allocator(&mut allocator)
            .ok()
            .flatten();
        if result.is_some() {
            *next_id = allocator.next_counter();
        }
        result
    }

    /// Allocator-aware form of [`Self::group_selected`].
    pub fn group_selected_with_allocator(
        &mut self,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        if self.selection.set.is_empty() {
            return Ok(None);
        }
        let selected = self.selection.set.clone();
        // Resolve a structural path, not merely a parent ID: an arbitrary
        // public document can contain duplicate IDs in different containers.
        let mut parent = None;
        let mut indices: Vec<usize> = Vec::with_capacity(selected.len());
        for id in &selected {
            let Some((path, index)) = sibling_location(self.active_children(), id, &mut Vec::new())
            else {
                return Ok(None);
            };
            if parent.as_ref().is_some_and(|parent| parent != &path) || indices.contains(&index) {
                return Ok(None);
            }
            parent.get_or_insert(path);
            indices.push(index);
        }
        indices.sort_unstable();
        let mut live = self.collect_node_ids();
        let group_id = allocator.allocate(&mut live)?;
        let children = siblings_at_path_mut(self.active_children_mut(), &parent.unwrap())
            .expect("validated common sibling array");
        let insert_at = indices[0];
        // Drain in reverse so removal indices stay valid.
        let mut taken: Vec<PenNode> = Vec::with_capacity(indices.len());
        for idx in indices.iter().rev() {
            taken.push(children.remove(*idx));
        }
        taken.reverse();
        let group = make_group(group_id.clone().into(), "Group", taken);
        children.insert(insert_at, group);
        self.selection.set = vec![group_id.clone()];
        self.selection.anchor = group_id.clone();
        Ok(Some(group_id))
    }

    /// Replace the selected `Group` node with its children inline.
    /// No-op when the selection isn't a single root or nested Group.
    pub fn ungroup_selected(&mut self) -> bool {
        if self.selection.set.len() != 1 {
            return false;
        }
        let target = self.selection.anchor.clone();
        let Some((path, idx)) = sibling_location(self.active_children(), &target, &mut Vec::new())
        else {
            return false;
        };
        let children = siblings_at_path_mut(self.active_children_mut(), &path)
            .expect("validated containing sibling array");
        if !children[idx].is_group() {
            return false;
        }
        let PenNode::Group(group) = children.remove(idx) else {
            unreachable!()
        };
        // Move, rather than deep-clone, every complete canonical child record.
        let inner = group.children.unwrap_or_default();
        let new_ids: Vec<NodeId> = inner
            .iter()
            .filter_map(|c| NodeId::new_opt(c.id_str()))
            .collect();
        for (k, child) in inner.into_iter().enumerate() {
            children.insert(idx + k, child);
        }
        if new_ids.is_empty() {
            self.clear_selection();
        } else {
            self.selection.anchor = new_ids.last().cloned().unwrap();
            self.selection.set = new_ids;
        }
        true
    }
}

/// Parent path + sibling index. Root matches retain the previous flat priority.
fn sibling_location(
    nodes: &[PenNode],
    target: &NodeId,
    path: &mut Vec<usize>,
) -> Option<(Vec<usize>, usize)> {
    if let Some(index) = nodes
        .iter()
        .position(|node| node.id_str() == target.as_str())
    {
        return Some((path.clone(), index));
    }
    for (index, node) in nodes.iter().enumerate() {
        if let Some(children) = node.children() {
            path.push(index);
            if let Some(found) = sibling_location(children, target, path) {
                return Some(found);
            }
            path.pop();
        }
    }
    None
}

fn siblings_at_path_mut<'a>(
    nodes: &'a mut Vec<PenNode>,
    path: &[usize],
) -> Option<&'a mut Vec<PenNode>> {
    let Some((&index, tail)) = path.split_first() else {
        return Some(nodes);
    };
    let parent = nodes.get_mut(index)?;
    // Looking for the parent must not author an absent array on a failed path.
    if parent.children().is_none() {
        return None;
    }
    siblings_at_path_mut(parent.children_mut()?, tail)
}

#[cfg(test)]
#[path = "grouping_nested_tests.rs"]
mod nested_tests;
