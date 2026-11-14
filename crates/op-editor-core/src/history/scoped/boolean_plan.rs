//! Borrowed Boolean footprint. Only operand leaves, their parents and anchors
//! are owned after the complete allocation forecast succeeds.
use super::{identity, patch, Scope};
use crate::{BooleanPathReplacement, EditorCommand as C, EditorState, PenNodeExt};
use jian_ops_schema::node::PenNode;
use std::{
    collections::{HashMap, HashSet},
    mem,
};
const LIMIT: usize = crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE;
pub(super) struct Leaf<'a> {
    pub node: &'a PenNode,
    pub parent: Box<[usize]>,
    pub parent_id: Option<&'a str>,
    pub parent_kind: Option<mem::Discriminant<PenNode>>,
    pub index: usize,
    pub total: usize,
    pub left: Option<&'a str>,
    pub right: Option<&'a str>,
}
pub(super) struct Plan<'a> {
    pub leaves: Vec<Leaf<'a>>,
    pub scope: Scope,
    pub root_len: usize,
    pub root_removed: usize,
    pub root_left: Option<&'a str>,
    pub forecast: usize,
}
fn add(total: &mut usize, bytes: usize) -> Option<()> {
    *total = total.checked_add(bytes)?;
    (*total <= LIMIT).then_some(())
}
// Splice exchange owns sibling identities/order tokens, but moves existing
// PenNode records. Forecast that working set and the possible Vec relocation
// before allocating any per-list identity map or owning operand payloads.
fn list_bytes(nodes: &[PenNode], extra: usize) -> Option<usize> {
    let shell = mem::size_of::<PenNode>().checked_mul(2)?.checked_add(512)?;
    let mut bytes = nodes.len().checked_add(extra)?.checked_mul(shell)?;
    for node in nodes {
        bytes = bytes.checked_add(node.id_str().len().checked_mul(32)?)?;
        if bytes > LIMIT {
            return None;
        }
    }
    Some(bytes)
}
fn request<'a>(state: &EditorState, command: &'a C) -> Option<&'a BooleanPathReplacement> {
    let C::ReplacePathsWithSvg(value) = command else {
        return None;
    };
    if state.editor_ui.workspace.active
        || state.instance_write_virtual_anchor.is_some()
        || value.document_generation != state.document_generation()
        || value.document_revision != state.document_revision()
        || value.active_page_index != state.ui.active_page_index
        || value.source_ids != state.selection.set
        || !(2..=512).contains(&value.source_ids.len())
        || value.d.trim().is_empty()
        || value.d.len() > 8 * 1024 * 1024
        || ![
            value.bounds.0,
            value.bounds.1,
            value.bounds.2,
            value.bounds.3,
        ]
        .iter()
        .all(|v| v.is_finite())
        || value.bounds.2 < value.bounds.0
        || value.bounds.3 < value.bounds.1
    {
        return None;
    }
    Some(value)
}
struct Frame<'a> {
    nodes: &'a [PenNode],
    next: usize,
    parent: Option<&'a PenNode>,
    blocked: bool,
    reference: bool,
}
pub(super) fn build<'a>(state: &'a EditorState, command: &C) -> Option<Plan<'a>> {
    let value = request(state, command)?;
    let wanted = value
        .source_ids
        .iter()
        .map(|id| id.as_str())
        .collect::<HashSet<_>>();
    if wanted.len() != value.source_ids.len() || wanted.contains("") {
        return None;
    }
    let scope = if let Some(pages) = state.doc.pages.as_ref().filter(|pages| !pages.is_empty()) {
        let index = state.ui.active_page_index;
        Scope::Page {
            index,
            id: pages.get(index)?.id.clone(),
        }
    } else {
        Scope::Root
    };
    let roots = patch::roots(&state.doc, &scope).ok()?;
    let mut bytes = 256 * 1024;
    add(&mut bytes, value.d.len().checked_mul(8)?)?;
    add(&mut bytes, list_bytes(roots, 1)?)?;
    let mut found = HashMap::with_capacity(wanted.len());
    let mut path = Vec::new();
    let mut stack = vec![Frame {
        nodes: roots,
        next: 0,
        parent: None,
        blocked: false,
        reference: false,
    }];
    while let Some(frame) = stack.last_mut() {
        if frame.next == frame.nodes.len() {
            stack.pop();
            if !stack.is_empty() {
                path.pop();
            }
            continue;
        }
        let at = frame.next;
        frame.next += 1;
        let node = &frame.nodes[at];
        path.push(at);
        let blocked =
            frame.blocked || node.base().locked == Some(true) || node.base().visible == Some(false);
        let reference = frame.reference || matches!(node, PenNode::Ref(_));
        if wanted.contains(node.id_str()) {
            if blocked
                || reference
                || found.contains_key(node.id_str())
                || node.children().is_some_and(|children| !children.is_empty())
                || !matches!(
                    node,
                    PenNode::Frame(_)
                        | PenNode::Rectangle(_)
                        | PenNode::Ellipse(_)
                        | PenNode::Polygon(_)
                        | PenNode::Path(_)
                        | PenNode::Line(_)
                )
            {
                return None;
            }
            let parent = frame.parent;
            if parent.is_some_and(|parent| !parent.is_container()) {
                return None;
            }
            let left = at.checked_sub(1).map(|at| frame.nodes[at].id_str());
            let right = frame.nodes.get(at + 1).map(PenNodeExt::id_str);
            let scalar = path
                .len()
                .checked_mul(mem::size_of::<usize>() * 3)?
                .checked_add(node.id_str().len().checked_mul(8)?)?;
            add(&mut bytes, scalar)?;
            for id in [parent.map(PenNodeExt::id_str), left, right]
                .into_iter()
                .flatten()
            {
                add(&mut bytes, id.len().checked_mul(4)?)?;
            }
            let payload = crate::history_snapshot::command_capture_node_bytes(
                node,
                false,
                LIMIT.checked_sub(bytes)?,
            )
            .ok()?;
            add(&mut bytes, payload)?;
            found.insert(
                node.id_str(),
                Leaf {
                    node,
                    parent: path[..path.len() - 1].into(),
                    parent_id: parent.map(PenNodeExt::id_str),
                    parent_kind: parent.map(mem::discriminant),
                    index: at,
                    total: frame.nodes.len(),
                    left,
                    right,
                },
            );
        }
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            stack.push(Frame {
                nodes: children,
                next: 0,
                parent: Some(node),
                blocked,
                reference,
            });
        } else {
            path.pop();
        }
    }
    if found.len() != wanted.len() {
        return None;
    }
    // Restoring an operand must not alias an existing canonical ID on another
    // page. Ambiguous inputs keep the legacy broad compatibility capture.
    let mut global = HashSet::with_capacity(wanted.len());
    let mut duplicate = false;
    let mut visit = |node: &PenNode, _: &[usize]| {
        if wanted.contains(node.id_str()) && !global.insert(node.id_str().to_owned()) {
            duplicate = true;
        }
    };
    identity::walk(&state.doc.children, &mut visit);
    if let Some(pages) = &state.doc.pages {
        for page in pages {
            identity::walk(&page.children, &mut visit);
        }
    }
    if duplicate {
        return None;
    }
    let leaves = value
        .source_ids
        .iter()
        .map(|id| found.remove(id.as_str()).unwrap())
        .collect::<Vec<_>>();
    let mut wanted_parents = identity::Wanted::new();
    let mut lists = HashMap::new();
    for leaf in &leaves {
        if let Some(id) = leaf.parent_id {
            wanted_parents
                .entry(scope.clone())
                .or_default()
                .insert(id.to_owned());
        }
        if lists.contains_key(leaf.parent.as_ref()) {
            continue;
        }
        let nodes = if leaf.parent.is_empty() {
            roots
        } else {
            patch::node(roots, &leaf.parent)?.children()?.as_slice()
        };
        if !leaf.parent.is_empty() {
            add(&mut bytes, list_bytes(nodes, leaves.len())?)?;
        }
        // Only relevant neighbors need ambiguity counts; a huge unrelated
        // sibling list does not require one owned map entry per node.
        let mut counts = HashMap::new();
        for related in leaves
            .iter()
            .filter(|related| related.parent == leaf.parent)
        {
            for id in [related.left, related.right].into_iter().flatten() {
                counts.entry(id).or_insert(0usize);
            }
        }
        for node in nodes {
            if let Some(count) = counts.get_mut(node.id_str()) {
                *count += 1;
            }
        }
        lists.insert(leaf.parent.clone(), counts);
    }
    let index = identity::Index::new(&state.doc, wanted_parents).ok()?;
    for leaf in &leaves {
        if let Some(id) = leaf.parent_id {
            if index.path(&scope, id).ok()?.as_ref() != leaf.parent.as_ref() {
                return None;
            }
        }
        let counts = &lists[leaf.parent.as_ref()];
        if [leaf.left, leaf.right]
            .into_iter()
            .flatten()
            .any(|id| counts.get(id) != Some(&1))
        {
            return None;
        }
    }
    let root_removed = leaves.iter().filter(|leaf| leaf.parent.is_empty()).count();
    let root_left = roots
        .iter()
        .rev()
        .find(|node| !wanted.contains(node.id_str()))
        .map(PenNodeExt::id_str);
    if let Some(left) = root_left {
        if roots.iter().filter(|node| node.id_str() == left).count() != 1 {
            return None;
        }
    }
    Some(Plan {
        leaves,
        scope,
        root_len: roots.len(),
        root_removed,
        root_left,
        forecast: bytes,
    })
}
pub(super) fn checked_request(
    state: &EditorState,
    command: &C,
    ids: &[crate::NodeId],
    generation: u64,
    revision: u64,
    page: usize,
) -> bool {
    request(state, command).is_some_and(|value| {
        value.source_ids == ids
            && value.document_generation == generation
            && value.document_revision == revision
            && value.active_page_index == page
    })
}
