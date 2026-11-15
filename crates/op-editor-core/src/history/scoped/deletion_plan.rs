//! Borrowed multi-selection deletion footprint. Lock protection and ancestor
//! selection mirror the linear production delete; actual ambiguity falls back.
use super::{identity, patch, Scope};
use crate::{EditorCommand as C, EditorState, NodeId, PenNodeExt};
use jian_ops_schema::node::PenNode;
use std::{
    collections::{HashMap, HashSet},
    mem,
};
const LIMIT: usize = crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE;
pub(super) struct Target<'a> {
    pub node: &'a PenNode,
    pub path: Box<[usize]>,
    pub parent: Option<&'a PenNode>,
    pub siblings: &'a [PenNode],
    pub index: usize,
}
pub(super) struct Plan<'a> {
    pub scope: Scope,
    pub targets: Vec<Target<'a>>,
    pub forecast: usize,
}
struct Candidate<'a> {
    target: Target<'a>,
    unlocked: bool,
    reference: bool,
}
struct Frame<'a> {
    node: Option<&'a PenNode>,
    parent: Option<&'a PenNode>,
    siblings: &'a [PenNode],
    index: usize,
    nodes: &'a [PenNode],
    next: usize,
    unlocked: bool,
    reference: bool,
}
fn add(bytes: &mut usize, amount: usize) -> Option<()> {
    *bytes = bytes.checked_add(amount)?;
    (*bytes <= LIMIT).then_some(())
}
fn list_bytes(nodes: &[PenNode], extra: usize) -> Option<usize> {
    let mut bytes = nodes
        .len()
        .checked_add(extra)?
        .checked_mul(mem::size_of::<PenNode>().checked_mul(2)?.checked_add(512)?)?;
    for node in nodes {
        bytes = bytes.checked_add(node.id_str().len().checked_mul(32)?)?;
        if bytes > LIMIT {
            return None;
        }
    }
    Some(bytes)
}
pub(super) fn build<'a>(state: &'a EditorState, command: &C) -> Option<Plan<'a>> {
    if !matches!(command, C::DeleteSelected | C::CutSelected)
        || state.editor_ui.workspace.active
        || state.instance_write_virtual_anchor.is_some()
    {
        return None;
    }
    let scope = match state.doc.pages.as_ref().filter(|pages| !pages.is_empty()) {
        Some(pages) => {
            let index = state.ui.active_page_index;
            Scope::Page {
                index,
                id: pages.get(index)?.id.clone(),
            }
        }
        None => Scope::Root,
    };
    let roots = patch::roots(&state.doc, &scope).ok()?;
    let wanted = state
        .selection
        .set
        .iter()
        .map(NodeId::as_str)
        .collect::<HashSet<_>>();
    let mut found = HashMap::with_capacity(wanted.len());
    let mut path = Vec::new();
    let mut bytes = 128 * 1024usize;
    let mut stack = vec![Frame {
        node: None,
        parent: None,
        siblings: roots,
        index: 0,
        nodes: roots,
        next: 0,
        unlocked: true,
        reference: false,
    }];
    while let Some(frame) = stack.last_mut() {
        if frame.next < frame.nodes.len() {
            let at = frame.next;
            frame.next += 1;
            let node = &frame.nodes[at];
            path.push(at);
            let next = Frame {
                node: Some(node),
                parent: frame.node,
                siblings: frame.nodes,
                index: at,
                nodes: node.children().map_or(&[][..], Vec::as_slice),
                next: 0,
                unlocked: node.base().locked != Some(true),
                reference: frame.reference || matches!(node, PenNode::Ref(_)),
            };
            stack.push(next);
            continue;
        }
        let completed = stack.pop().unwrap();
        if let Some(node) = completed.node {
            if wanted.contains(node.id_str()) {
                if found.contains_key(node.id_str()) {
                    return None;
                }
                add(
                    &mut bytes,
                    path.len()
                        .checked_mul(mem::size_of::<usize>() * 4)?
                        .checked_add(node.id_str().len().checked_mul(16)?)?,
                )?;
                found.insert(
                    node.id_str(),
                    Candidate {
                        target: Target {
                            node,
                            path: path.clone().into_boxed_slice(),
                            parent: completed.parent,
                            siblings: completed.siblings,
                            index: completed.index,
                        },
                        unlocked: completed.unlocked,
                        reference: completed.reference,
                    },
                );
            }
            path.pop();
        }
        if let Some(parent) = stack.last_mut() {
            parent.unlocked &= completed.unlocked;
        }
    }
    if matches!(command, C::CutSelected) {
        // The old clipboard remains retained until durable completion. Copy's
        // output includes protected selections and caller duplicates as before.
        let old = crate::history_snapshot::command_capture_value_bytes(
            &state.clipboard,
            LIMIT.checked_sub(bytes)?,
        )
        .ok()?;
        add(&mut bytes, old)?;
        for id in &state.selection.set {
            if let Some(candidate) = found.get(id.as_str()) {
                let output = crate::history_snapshot::command_capture_node_bytes(
                    candidate.target.node,
                    false,
                    LIMIT.checked_sub(bytes)?,
                )
                .ok()?;
                add(&mut bytes, output)?;
            }
        }
    }
    let mut candidates = found
        .into_values()
        .filter(|candidate| candidate.unlocked)
        .collect::<Vec<_>>();
    candidates.sort_unstable_by(|a, b| a.target.path.cmp(&b.target.path));
    let mut targets: Vec<Target<'a>> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if targets
            .last()
            .is_some_and(|ancestor| candidate.target.path.starts_with(&ancestor.path))
        {
            continue;
        }
        if candidate.reference
            || candidate
                .target
                .parent
                .is_some_and(|parent| !parent.is_container())
        {
            return None;
        }
        targets.push(candidate.target);
    }
    let mut changed = HashSet::new();
    let mut parents = identity::Wanted::new();
    let mut lists = HashMap::new();
    for target in &targets {
        let payload = crate::history_snapshot::command_capture_node_bytes(
            target.node,
            false,
            LIMIT.checked_sub(bytes)?,
        )
        .ok()?;
        add(&mut bytes, payload)?;
        if !changed.insert(target.node.id_str().to_owned()) {
            return None;
        }
        identity::walk(
            target.node.children().map_or(&[][..], Vec::as_slice),
            |node, _| {
                changed.insert(node.id_str().to_owned());
            },
        );
        if let Some(parent) = target.parent {
            parents
                .entry(scope.clone())
                .or_default()
                .insert(parent.id_str().to_owned());
        }
        let parent_path = &target.path[..target.path.len() - 1];
        if lists.contains_key(parent_path) {
            continue;
        }
        add(&mut bytes, list_bytes(target.siblings, targets.len())?)?;
        let mut counts = HashMap::new();
        for related in targets
            .iter()
            .filter(|related| related.path[..related.path.len() - 1] == *parent_path)
        {
            for node in [
                related
                    .index
                    .checked_sub(1)
                    .and_then(|at| related.siblings.get(at)),
                related.siblings.get(related.index + 1),
            ]
            .into_iter()
            .flatten()
            {
                counts.entry(node.id_str()).or_insert(0usize);
            }
        }
        for node in target.siblings {
            if let Some(count) = counts.get_mut(node.id_str()) {
                *count += 1;
            }
        }
        if counts.values().any(|count| *count != 1) {
            return None;
        }
        lists.insert(parent_path.to_vec().into_boxed_slice(), ());
    }
    let mut occurrences = HashSet::with_capacity(changed.len());
    let mut collision = false;
    let mut visit = |node: &PenNode, _: &[usize]| {
        if changed.contains(node.id_str()) && !occurrences.insert(node.id_str().to_owned()) {
            collision = true;
        }
    };
    identity::walk(&state.doc.children, &mut visit);
    if let Some(pages) = &state.doc.pages {
        for page in pages {
            identity::walk(&page.children, &mut visit);
        }
    }
    if collision {
        return None;
    }
    let index = identity::Index::new(&state.doc, parents).ok()?;
    for target in &targets {
        if let Some(parent) = target.parent {
            if index.path(&scope, parent.id_str()).ok()?.as_ref()
                != &target.path[..target.path.len() - 1]
            {
                return None;
            }
        }
    }
    Some(Plan {
        scope,
        targets,
        forecast: bytes,
    })
}
