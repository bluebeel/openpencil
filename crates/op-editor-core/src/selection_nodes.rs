//! Resolve only a requested selection with one borrowed preorder traversal.
use crate::{NodeId, PenNodeExt};
use jian_ops_schema::node::PenNode;
use std::collections::{HashMap, HashSet};

/// Preserve first-preorder duplicate-ID lookup, caller order and missing IDs.
/// The result borrows the document; unrelated node payloads are never cloned.
pub(crate) fn resolve<'a>(
    nodes: &'a [PenNode],
    selection: &[NodeId],
) -> HashMap<&'a str, &'a PenNode> {
    let wanted: HashSet<&str> = selection.iter().map(NodeId::as_str).collect();
    let mut found = HashMap::with_capacity(wanted.len());
    if wanted.is_empty() {
        return found;
    }
    let mut stack = vec![nodes.iter()];
    while let Some(siblings) = stack.last_mut() {
        let Some(node) = siblings.next() else {
            stack.pop();
            continue;
        };
        if wanted.contains(node.id_str()) {
            found.entry(node.id_str()).or_insert(node);
            if found.len() == wanted.len() {
                break;
            }
        }
        if let Some(children) = node.children() {
            if !children.is_empty() {
                stack.push(children.iter());
            }
        }
    }
    found
}

/// Linear delete for unambiguous selected IDs. Duplicate selected identities
/// keep the legacy resolver/removal order rather than choosing a new authority.
pub(crate) fn delete(state: &mut crate::EditorState) -> Option<bool> {
    struct Frame<'a> {
        id: Option<&'a str>,
        children: std::slice::Iter<'a, PenNode>,
        unlocked: bool,
    }
    let (deletable, kept) = {
        let wanted: HashSet<&str> = state.selection.set.iter().map(NodeId::as_str).collect();
        let mut seen = HashSet::with_capacity(wanted.len());
        let mut unlocked_ids = HashSet::with_capacity(wanted.len());
        let mut stack = vec![Frame {
            id: None,
            children: state.active_children().iter(),
            unlocked: true,
        }];
        while let Some(frame) = stack.last_mut() {
            if let Some(node) = frame.children.next() {
                let id = node.id_str();
                if wanted.contains(id) && !seen.insert(id) {
                    return None;
                }
                stack.push(Frame {
                    id: Some(id),
                    children: node.children().map_or(&[][..], Vec::as_slice).iter(),
                    unlocked: !node.base().locked.unwrap_or(false),
                });
            } else {
                let completed = stack.pop().expect("a current frame exists");
                if let Some(id) = completed.id.filter(|id| wanted.contains(id)) {
                    if completed.unlocked {
                        unlocked_ids.insert(id);
                    }
                }
                if let Some(parent) = stack.last_mut() {
                    parent.unlocked &= completed.unlocked;
                }
            }
        }
        let kept = state
            .selection
            .set
            .iter()
            .filter(|id| !unlocked_ids.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let deletable = unlocked_ids
            .into_iter()
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        (deletable, kept)
    };
    if deletable.is_empty() {
        return Some(false);
    }
    fn remove(nodes: &mut Vec<PenNode>, ids: &HashSet<String>) -> bool {
        let mut removed = false;
        nodes.retain_mut(|node| {
            if ids.contains(node.id_str()) {
                removed = true;
                return false;
            }
            if node.children().is_some() {
                if let Some(children) = node.children_mut() {
                    removed |= remove(children, ids);
                }
            }
            true
        });
        removed
    }
    let removed = remove(state.active_children_mut(), &deletable);
    if removed {
        state.selection.set = kept;
        state.selection.anchor = state.selection.set.last().cloned().unwrap_or(NodeId::NONE);
    }
    Some(removed)
}
