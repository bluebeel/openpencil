//! Linear removal for unambiguous leaf operands. Public callers with duplicate
//! identities or selected subtrees retain the historical ordered removals.

use crate::{NodeId, PenNodeExt};
use jian_ops_schema::node::PenNode;
use std::collections::HashSet;

pub(super) fn remove_unambiguous_leaves(roots: &mut Vec<PenNode>, ids: &[NodeId]) -> bool {
    let wanted: HashSet<&str> = ids.iter().map(NodeId::as_str).collect();
    if wanted.is_empty() {
        return true;
    }
    {
        let mut seen = HashSet::with_capacity(wanted.len());
        let mut stack = vec![roots.iter()];
        while let Some(siblings) = stack.last_mut() {
            let Some(node) = siblings.next() else {
                stack.pop();
                continue;
            };
            if wanted.contains(node.id_str())
                && (!seen.insert(node.id_str())
                    || node.children().is_some_and(|children| !children.is_empty()))
            {
                return false;
            }
            if let Some(children) = node.children().filter(|children| !children.is_empty()) {
                stack.push(children.iter());
            }
        }
    }
    fn remove(nodes: &mut Vec<PenNode>, wanted: &HashSet<&str>) {
        nodes.retain_mut(|node| {
            if wanted.contains(node.id_str()) {
                return false;
            }
            // Searching must preserve absent versus present-empty child lists.
            if node.children().is_some() {
                if let Some(children) = node.children_mut() {
                    remove(children, wanted);
                }
            }
            true
        });
    }
    remove(roots, &wanted);
    true
}
