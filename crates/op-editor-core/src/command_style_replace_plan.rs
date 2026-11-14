//! Borrowed style previews and stable routes; no forest-sized dry-run copy.

use super::{apply_replacement, validate_replacements, StyleReplaceError};
use crate::{pen_node_ext::clone_without_children, NodeId, PenNodeExt, StylePropertyReplacement};
use jian_ops_schema::node::PenNode;
use std::collections::{HashMap, HashSet};

/// Match the mutating operation's count, including chained rules and repeated
/// or overlapping parent subtrees, while leaving canonical data untouched.
pub fn preview_matching_properties_in_roots(
    roots: &[PenNode],
    parent_ids: &[NodeId],
    replacements: &[StylePropertyReplacement],
) -> Result<usize, StyleReplaceError> {
    validate_replacements(replacements)?;
    if replacements.is_empty() {
        return Ok(0);
    }
    let found = crate::selection_nodes::resolve(roots, parent_ids);
    // Source addresses distinguish authored duplicate IDs. They are local
    // identity keys only; no pointer is dereferenced, retained or returned.
    let mut changed: HashMap<usize, PenNode> = HashMap::new();
    let mut count = 0;
    for id in parent_ids.iter().filter(|id| id.is_real()) {
        let Some(&parent) = found.get(id.as_str()) else {
            continue;
        };
        let mut pending = vec![parent];
        while let Some(node) = pending.pop() {
            let key = node as *const PenNode as usize;
            if let Some(header) = changed.get_mut(&key) {
                for replacement in replacements {
                    count += usize::from(apply_replacement(header, replacement));
                }
            } else {
                let mut header = clone_without_children(node);
                let mut node_count = 0;
                for replacement in replacements {
                    node_count += usize::from(apply_replacement(&mut header, replacement));
                }
                count += node_count;
                if node_count != 0 {
                    changed.insert(key, header);
                }
            }
            if let Some(children) = node.children() {
                pending.extend(children.iter().rev());
            }
        }
    }
    Ok(count)
}

pub(super) fn parent_routes(roots: &[PenNode], ids: &[NodeId]) -> HashMap<String, Vec<usize>> {
    let wanted: HashSet<&str> = ids
        .iter()
        .filter(|id| id.is_real())
        .map(NodeId::as_str)
        .collect();
    let mut found = HashMap::with_capacity(wanted.len());
    if wanted.is_empty() {
        return found;
    }
    let mut stack = vec![(0usize, roots)];
    let mut path = Vec::new();
    while let Some((next, siblings)) = stack.last_mut() {
        let siblings = *siblings;
        let Some(node) = siblings.get(*next) else {
            stack.pop();
            path.pop();
            continue;
        };
        path.push(*next);
        *next += 1;
        if wanted.contains(node.id_str()) && !found.contains_key(node.id_str()) {
            found.insert(node.id_str().to_owned(), path.clone());
            if found.len() == wanted.len() {
                break;
            }
        }
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            stack.push((0, children));
        } else {
            path.pop();
        }
    }
    found
}

pub(super) fn node_mut<'a>(roots: &'a mut [PenNode], path: &[usize]) -> Option<&'a mut PenNode> {
    let (index, rest) = path.split_first()?;
    let node = roots.get_mut(*index)?;
    if rest.is_empty() {
        return Some(node);
    }
    if node.children().is_none() {
        return None;
    }
    node_mut(node.children_mut()?, rest)
}
