//! Allocation bounds before preparing intended clone payloads or rollback IDs.
use super::{lookup, paste_target, reusable};
use crate::{
    history_snapshot::{
        command_capture_node_bytes, command_capture_value_bytes, COMMAND_CAPTURE_ALLOWANCE,
    },
    EditorCommand as C, EditorState, PenNodeExt,
};
use jian_ops_schema::node::PenNode;
use std::{
    collections::{HashMap, HashSet},
    mem,
};
const LIMIT: usize = COMMAND_CAPTURE_ALLOWANCE;
fn add(total: &mut usize, bytes: usize) -> Option<()> {
    *total = total.checked_add(bytes)?;
    (*total <= LIMIT).then_some(())
}
fn payload(node: &PenNode) -> Option<usize> {
    command_capture_node_bytes(node, false, LIMIT).ok()
}
fn parent<'a>(roots: &'a [PenNode], path: &[usize]) -> Option<(&'a [PenNode], usize)> {
    if path.is_empty() {
        Some((roots, roots.len()))
    } else {
        let nodes = lookup::at(roots, path)?.children()?;
        Some((nodes, nodes.capacity()))
    }
}
pub(super) fn bound(state: &EditorState, command: &C) -> Option<usize> {
    let roots = state.active_children();
    let mut total = 256 * 1024;
    let mut parents: HashMap<Box<[usize]>, usize> = HashMap::new();
    match command {
        C::DuplicateSelected { .. } => {
            let wanted: HashSet<_> = state
                .selection
                .set
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect();
            let index = lookup::Index::for_state(state, wanted);
            if state
                .selection
                .set
                .iter()
                .any(|id| index.ambiguous(id.as_str()))
            {
                return None;
            }
            let mut outputs: Vec<(Box<[usize]>, usize)> = Vec::new();
            let mut largest = 0;
            for id in &state.selection.set {
                let bytes = if let Some(source) = index.original.get(id.as_str()) {
                    let node = lookup::at(roots, &source.sibling_first)?;
                    let bytes = if reusable(node) {
                        16 * 1024
                    } else {
                        let mut bytes = payload(node)?;
                        for (parent, inserted) in &outputs {
                            if parent.starts_with(&source.sibling_first) {
                                add(&mut bytes, *inserted)?;
                            }
                        }
                        bytes
                    };
                    let parent = source.sibling_first[..source.sibling_first.len() - 1].into();
                    *parents.entry(parent).or_default() += 1;
                    bytes
                } else {
                    // A previously missing requested id may be minted by an
                    // earlier output. Its subtree cannot exceed that output.
                    let bytes = largest;
                    for (_, prior) in &mut outputs {
                        *prior = prior.checked_add(bytes)?;
                    }
                    if bytes > 0 {
                        for count in parents.values_mut() {
                            *count += 1;
                        }
                        largest = largest.checked_add(bytes)?;
                    }
                    bytes
                };
                add(&mut total, bytes)?;
                largest = largest.max(bytes);
                if let Some(source) = index.original.get(id.as_str()) {
                    outputs.push((
                        source.sibling_first[..source.sibling_first.len() - 1].into(),
                        bytes,
                    ));
                }
            }
        }
        C::PasteClipboard { .. } => {
            let mut wanted = HashSet::new();
            if let Some(anchor) = state.selection.set.first() {
                wanted.insert(anchor.as_str().to_owned());
            }
            for node in &state.clipboard {
                if reusable(node) {
                    wanted.insert(node.id_str().to_owned());
                }
            }
            let mut index = lookup::Index::for_state(state, wanted);
            let (destination, _) =
                paste_target(state, roots, &mut index, state.selection.set.first())?;
            for node in &state.clipboard {
                let live = index.original.get(node.id_str()).filter(|source| {
                    reusable(node) && reusable(lookup::at(roots, &source.preorder).unwrap())
                });
                let bytes = if let Some(source) = live {
                    if source.count > 1 {
                        return None;
                    }
                    *parents
                        .entry(source.sibling_first[..source.sibling_first.len() - 1].into())
                        .or_default() += 1;
                    16 * 1024
                } else {
                    *parents.entry(destination.clone()).or_default() += 1;
                    payload(node)?
                };
                add(&mut total, bytes)?;
            }
            // Exact clipboard comparison retains only wire bytes/sensitivity,
            // never another owned PenNode forest or a document replay buffer.
            add(
                &mut total,
                command_capture_value_bytes(&state.clipboard, LIMIT).ok()?,
            )?;
        }
        _ => return None,
    }
    for (path, count) in parents {
        let (nodes, capacity) = parent(roots, &path)?;
        let needed = nodes.len().checked_add(count)?;
        add(&mut total, needed.checked_mul(256)?)?;
        if needed > capacity {
            add(
                &mut total,
                needed
                    .checked_mul(mem::size_of::<PenNode>())?
                    .checked_mul(2)?,
            )?;
        }
    }
    Some(total)
}
