//! Structural proofs retain routes and identities, never moved descendants.
use super::{command_scope, splice};
use crate::{
    history::{Scope, TargetSpec, TopologyKind, TopologyLocation, TopologyRequest, TopologySpec},
    history_snapshot::{command_capture_node_bytes, COMMAND_CAPTURE_ALLOWANCE},
    walkers::{descendant_contains, ReorderDirection},
    EditorCommand as C, EditorState, NodeId, PenNodeExt,
};
use jian_ops_schema::node::PenNode;
use std::{collections::HashMap, mem};

const ROUTE_BYTES: usize = 512 * 1024;
fn bounded(bytes: usize) -> Option<usize> {
    (bytes <= COMMAND_CAPTURE_ALLOWANCE).then_some(bytes)
}
fn identity_bytes<'a>(mut ids: impl Iterator<Item = &'a str>) -> Option<usize> {
    ids.try_fold(0usize, |sum, id| {
        if id.len() > 16 * 1024 {
            return None;
        }
        bounded(sum.checked_add(4096 + id.len().checked_mul(32)?)?)
    })
}
fn at<'a>(roots: &'a [PenNode], path: &[usize]) -> Option<&'a PenNode> {
    let (head, tail) = path.split_first()?;
    let mut node = roots.get(*head)?;
    for index in tail {
        node = node.children()?.get(*index)?;
    }
    Some(node)
}
fn parent_ok(roots: &[PenNode], path: &[usize]) -> Option<()> {
    if path.len() > 128 {
        return None;
    }
    let mut nodes = roots;
    for index in path {
        let node = nodes.get(*index)?;
        if !node.is_container()
            || matches!(node, PenNode::Ref(_))
            || node.id_str().len() > 16 * 1024
        {
            return None;
        }
        nodes = node.children().map_or(&[], Vec::as_slice);
    }
    Some(())
}
fn siblings<'a>(roots: &'a [PenNode], parent: &[usize]) -> Option<&'a [PenNode]> {
    if parent.is_empty() {
        return Some(roots);
    }
    Some(at(roots, parent)?.children().map_or(&[], Vec::as_slice))
}
/// A Vec insert can relocate the complete sibling shell allocation. Include
/// that temporary peak before mutation; node payloads themselves are moved.
fn growth_bytes(
    state: &EditorState,
    scope: &Scope,
    parent: &[usize],
    extra: usize,
) -> Option<usize> {
    let roots = match scope {
        Scope::Root => &state.doc.children,
        Scope::Page { index, .. } => &state.doc.pages.as_ref()?[*index].children,
    };
    let (len, capacity) = if parent.is_empty() {
        (roots.len(), roots.capacity())
    } else {
        at(roots, parent)?
            .children()
            .map_or((0, 0), |nodes| (nodes.len(), nodes.capacity()))
    };
    let needed = len.checked_add(extra)?;
    if needed <= capacity {
        return Some(0);
    }
    bounded(
        needed
            .max(4)
            .checked_mul(2)?
            .checked_mul(mem::size_of::<PenNode>())?,
    )
}
fn target(
    scope: Scope,
    parent: Vec<usize>,
    request: TopologyRequest,
    kind: TopologyKind,
    forecast: usize,
) -> Option<TargetSpec> {
    Some(TargetSpec {
        scope,
        path: parent.into_boxed_slice(),
        subtree: false,
        splice: None,
        topology: Some(TopologySpec {
            request,
            kind,
            forecast: bounded(forecast)?,
        }),
    })
}
fn location(mut path: Vec<usize>) -> Option<TopologyLocation> {
    let index = path.pop()?;
    Some(TopologyLocation {
        parent: path.into_boxed_slice(),
        index,
    })
}
/// The reorder walker returns early for an immediate match at an edge, while
/// its caller may continue into another sibling subtree containing the same id.
fn reorder_path(
    nodes: &[PenNode],
    id: &NodeId,
    direction: ReorderDirection,
) -> Option<(Vec<usize>, usize)> {
    if let Some(index) = nodes.iter().position(|node| node.id_str() == id.as_str()) {
        let to = match direction {
            ReorderDirection::Up => index.checked_sub(1)?,
            ReorderDirection::Down if index + 1 < nodes.len() => index + 1,
            _ => return None,
        };
        return Some((vec![index], to));
    }
    for (index, node) in nodes.iter().enumerate() {
        if let Some((tail, to)) = node
            .children()
            .and_then(|children| reorder_path(children, id, direction))
        {
            let mut path = Vec::with_capacity(tail.len() + 1);
            path.push(index);
            path.extend(tail);
            return Some((path, to));
        }
    }
    None
}
/// Match grouping's sibling-first priority in one borrowed tree traversal for
/// every selected id, rather than repeatedly scanning the document per member.
fn group_locations(roots: &[PenNode], selected: &[NodeId]) -> Option<Vec<TopologyLocation>> {
    let mut pending: HashMap<&str, usize> = HashMap::with_capacity(selected.len());
    for (index, id) in selected.iter().enumerate() {
        if !id.is_real() || pending.insert(id.as_str(), index).is_some() {
            return None;
        }
    }
    let mut found = vec![None; selected.len()];
    fn visit(
        nodes: &[PenNode],
        path: &mut Vec<usize>,
        pending: &mut HashMap<&str, usize>,
        found: &mut [Option<TopologyLocation>],
    ) {
        for (index, node) in nodes.iter().enumerate() {
            if let Some(slot) = pending.remove(node.id_str()) {
                found[slot] = Some(TopologyLocation {
                    parent: path.clone().into_boxed_slice(),
                    index,
                });
            }
        }
        if pending.is_empty() || path.len() >= 128 {
            return;
        }
        for (index, node) in nodes.iter().enumerate() {
            if let Some(children) = node.children() {
                path.push(index);
                visit(children, path, pending, found);
                path.pop();
                if pending.is_empty() {
                    return;
                }
            }
        }
    }
    visit(roots, &mut Vec::new(), &mut pending, &mut found);
    found.into_iter().collect()
}

pub(super) fn plan(state: &EditorState, command: &C, active: usize) -> Option<TargetSpec> {
    let raw = match command {
        C::MoveNode { page_id, .. } => page_id.as_deref(),
        C::ReorderSelected { .. } | C::GroupSelected | C::UngroupSelected => None,
        _ => return None,
    };
    let scope = command_scope::scope(state, command_scope::page_index(state, raw, active)?);
    let roots = match &scope {
        Scope::Root => &state.doc.children,
        Scope::Page { index, .. } => &state.doc.pages.as_ref()?[*index].children,
    };
    match command {
        C::MoveNode {
            node_id,
            target_parent,
            page_id,
            index,
        } => {
            if !node_id.is_real() || target_parent == node_id {
                return None;
            }
            let request_bytes =
                identity_bytes([node_id.as_str(), target_parent.as_str()].into_iter())?
                    .checked_add(identity_bytes(page_id.iter().map(String::as_str))?)?;
            let source_path = splice::read_path(roots, node_id)?;
            if splice::remove_path(roots, node_id)? != source_path {
                return None;
            }
            let source = at(roots, &source_path)?;
            let from = location(source_path)?;
            parent_ok(roots, &from.parent)?;
            let parent = if target_parent.is_real() {
                if descendant_contains(source, target_parent) {
                    return None;
                }
                let path = splice::read_path(roots, target_parent)?;
                if splice::remove_path(roots, target_parent)? != path {
                    return None;
                }
                parent_ok(roots, &path)?;
                path
            } else {
                Vec::new()
            };
            let same = from.parent.as_ref() == parent.as_slice();
            let count = siblings(roots, &parent)?
                .len()
                .checked_sub(usize::from(same))?;
            let to = TopologyLocation {
                parent: parent.clone().into_boxed_slice(),
                index: index.unwrap_or(count).min(count),
            };
            let growth = if same {
                0
            } else {
                growth_bytes(state, &scope, &parent, 1)?
            };
            let request = TopologyRequest::Move {
                node_id: node_id.clone(),
                parent: target_parent.clone(),
                page: page_id.clone(),
                index: *index,
            };
            let kind = TopologyKind::Move {
                from,
                to,
                node_id: node_id.as_str().to_owned(),
                no_op_is_success: true,
            };
            target(
                scope,
                parent,
                request,
                kind,
                ROUTE_BYTES
                    .checked_add(request_bytes)?
                    .checked_add(growth)?,
            )
        }
        C::ReorderSelected { direction } => {
            if !state.selection.anchor.is_real() {
                return None;
            }
            let (path, to) = reorder_path(roots, &state.selection.anchor, *direction)?;
            let from = location(path)?;
            parent_ok(roots, &from.parent)?;
            identity_bytes(std::iter::once(state.selection.anchor.as_str()))?;
            let parent = from.parent.to_vec();
            let request = TopologyRequest::Reorder {
                anchor: state.selection.anchor.clone(),
                direction: *direction,
            };
            let kind = TopologyKind::Move {
                from,
                to: TopologyLocation {
                    parent: parent.clone().into_boxed_slice(),
                    index: to,
                },
                node_id: state.selection.anchor.as_str().to_owned(),
                no_op_is_success: false,
            };
            target(scope, parent, request, kind, ROUTE_BYTES)
        }
        C::GroupSelected => {
            if state.selection.set.is_empty() {
                return None;
            }
            let ids = identity_bytes(
                state
                    .selection
                    .set
                    .iter()
                    .chain(std::iter::once(&state.selection.anchor))
                    .map(NodeId::as_str),
            )?;
            let found = group_locations(roots, &state.selection.set)?;
            let parent = found.first()?.parent.clone();
            parent_ok(roots, &parent)?;
            if found.iter().any(|item| item.parent != parent) {
                return None;
            }
            let mut positions = found.into_iter().map(|item| item.index).collect::<Vec<_>>();
            positions.sort_unstable();
            let shells = positions.len().checked_mul(mem::size_of::<PenNode>())?;
            let forecast = ROUTE_BYTES
                .checked_add(64 * 1024)?
                .checked_add(ids)?
                .checked_add(shells)?;
            target(
                scope,
                parent.into_vec(),
                TopologyRequest::Group(state.selection.clone()),
                TopologyKind::Group {
                    positions: positions.into_boxed_slice(),
                },
                forecast,
            )
        }
        C::UngroupSelected => {
            if state.selection.set.len() != 1 {
                return None;
            }
            identity_bytes(
                state
                    .selection
                    .set
                    .iter()
                    .chain(std::iter::once(&state.selection.anchor))
                    .map(NodeId::as_str),
            )?;
            let path = splice::remove_path(roots, &state.selection.anchor)?;
            let group = at(roots, &path)?;
            if !group.is_group() {
                return None;
            }
            let from = location(path)?;
            parent_ok(roots, &from.parent)?;
            let members = group.children().map_or(&[], Vec::as_slice);
            let ids = identity_bytes(members.iter().map(PenNodeExt::id_str))?;
            let header = command_capture_node_bytes(group, true, COMMAND_CAPTURE_ALLOWANCE).ok()?;
            let growth =
                growth_bytes(state, &scope, &from.parent, members.len().saturating_sub(1))?;
            let forecast = ROUTE_BYTES
                .checked_add(ids)?
                .checked_add(header)?
                .checked_add(growth)?;
            target(
                scope,
                from.parent.into_vec(),
                TopologyRequest::Ungroup(state.selection.clone()),
                TopologyKind::Ungroup { index: from.index },
                forecast,
            )
        }
        _ => None,
    }
}
