//! Group / ungroup mutators (`Cmd+G` / `Cmd+Shift+G` parity).
//!
//! Both ops resolve siblings under one parent on the active page. A group
//! inserts where its first member sat; ungroup splices its children back into
//! that same sibling array. Child records are moved without geometry changes.

use crate::id_allocator::{IdAllocError, IdAllocator};
use crate::node_id::NodeId;
use crate::pen_node_ext::{make_group, PenNodeExt};
use crate::state::EditorState;
use jian_ops_schema::node::PenNode;
use std::collections::HashSet;

impl EditorState {
    /// Wrap the current selection in a fresh `Group` node inserted
    /// at the position of the first selected sibling. Supports root or
    /// nested same-parent selections. Returns the new group's
    /// id, or `None` when the selection is empty / spans parents.
    pub fn group_selected(&mut self, next_id: &mut u64) -> Option<NodeId> {
        let mut allocator = self.sequential_id_allocator(*next_id).ok()?;
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
    pub(crate) fn group_selected_with_allocator_unobserved(
        &mut self,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        if self.selection.set.is_empty() {
            return Ok(None);
        }
        let Some((parent, indices)) = self.command_selected_siblings().unwrap_or_else(|| {
            selected_siblings(self.active_children(), &self.selection.set)
                .map(|(parent, indices)| (parent.into_boxed_slice(), indices))
        }) else {
            return Ok(None);
        };
        let mut live = allocator.document_collision_ids(&self.doc);
        let group_id = allocator.allocate(&mut live)?;
        let children = siblings_at_path_mut(self.active_children_mut(), &parent)
            .expect("validated common sibling array");
        let insert_at = indices[0];
        let taken = if indices.len() == children.len() {
            // Select-all can transfer the existing allocation directly.
            std::mem::take(children)
        } else {
            pack_selected(children, &indices);
            children
                .drain(insert_at..insert_at + indices.len())
                .collect()
        };
        children.insert(
            insert_at,
            make_group(group_id.clone().into(), "Group", taken),
        );
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
        let scope = self.command_lookup_active_scope();
        let indexed = self.command_lookup_path(
            &scope,
            &target,
            crate::command_id_scope::sequence::LookupOrder::SiblingFirst,
        );
        let found = match indexed {
            Some(path) => path.and_then(|path| {
                let (&slot, parent) = path.split_last()?;
                Some((parent.to_vec(), slot))
            }),
            None => sibling_location(self.active_children(), &target, &mut Vec::new()),
        };
        let Some((path, idx)) = found else {
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
        // Reserve/splice once, rather than shifting the entire tail per child.
        drop(children.splice(idx..idx, inner));
        if new_ids.is_empty() {
            self.clear_selection();
        } else {
            self.selection.anchor = new_ids.last().cloned().unwrap();
            self.selection.set = new_ids;
        }
        true
    }
}

/// Resolve the entire selection with the same sibling-before-descendant
/// identity precedence as sibling_location, without one forest scan per ID.
fn selected_siblings(nodes: &[PenNode], selected: &[NodeId]) -> Option<(Vec<usize>, Vec<usize>)> {
    let wanted: HashSet<&str> = selected.iter().map(NodeId::as_str).collect();
    if wanted.len() != selected.len() {
        return None;
    }
    struct Frame<'a> {
        nodes: &'a [PenNode],
        next_child: usize,
        scanned: bool,
    }
    let mut stack = vec![Frame {
        nodes,
        next_child: 0,
        scanned: false,
    }];
    let mut path = Vec::new();
    let mut parent: Option<Vec<usize>> = None;
    let mut seen = HashSet::with_capacity(wanted.len());
    let mut indices = Vec::with_capacity(wanted.len());
    while let Some(frame) = stack.last_mut() {
        if !frame.scanned {
            for (index, node) in frame.nodes.iter().enumerate() {
                let id = node.id_str();
                if wanted.contains(id) && seen.insert(id) {
                    if parent.as_ref().is_some_and(|parent| parent != &path) {
                        return None;
                    }
                    parent.get_or_insert_with(|| path.clone());
                    indices.push(index);
                }
            }
            if seen.len() == wanted.len() {
                break;
            }
            frame.scanned = true;
        }
        if frame.next_child == frame.nodes.len() {
            stack.pop();
            path.pop();
            continue;
        }
        let index = frame.next_child;
        frame.next_child += 1;
        if let Some(children) = frame.nodes[index]
            .children()
            .filter(|children| !children.is_empty())
        {
            path.push(index);
            stack.push(Frame {
                nodes: children,
                next_child: 0,
                scanned: false,
            });
        }
    }
    if seen.len() != wanted.len() {
        return None;
    }
    indices.sort_unstable();
    Some((parent?, indices))
}

/// One linear in-place permutation brings selected siblings together. Only a
/// usize mapping is allocated; complete canonical node records are never cloned.
fn pack_selected(nodes: &mut [PenNode], selected: &[usize]) {
    let first = selected[0];
    let mut positions = vec![0usize; nodes.len()];
    let (mut member, mut other) = (0, 0);
    for (index, destination) in positions.iter_mut().enumerate() {
        if selected.get(member) == Some(&index) {
            *destination = first + member;
            member += 1;
        } else {
            if other == first {
                other += selected.len();
            }
            *destination = other;
            other += 1;
        }
    }
    for index in 0..nodes.len() {
        while positions[index] != index {
            let destination = positions[index];
            nodes.swap(index, destination);
            positions.swap(index, destination);
        }
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
