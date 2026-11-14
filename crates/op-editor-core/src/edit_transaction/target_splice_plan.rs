//! Conservative command-specific structural ranges; dynamic batches fall back.
use super::command_scope;
use crate::{
    history::{structural_payload_bound, Scope, SpliceSpec, TargetSpec},
    EditorCommand as C, EditorState, NodeId, PenNodeExt,
};
use jian_ops_schema::node::PenNode;

/// Read/mutable node lookups are preorder; removal/insertion tree helpers first
/// inspect every immediate sibling. Duplicate ids must not redirect a capture.
pub(super) fn read_path(nodes: &[PenNode], id: &NodeId) -> Option<Vec<usize>> {
    for (index, node) in nodes.iter().enumerate() {
        if node.id_str() == id.as_str() {
            return Some(vec![index]);
        }
        if let Some(tail) = node.children().and_then(|children| read_path(children, id)) {
            let mut path = Vec::with_capacity(tail.len() + 1);
            path.push(index);
            path.extend(tail);
            return Some(path);
        }
    }
    None
}
pub(super) fn remove_path(nodes: &[PenNode], id: &NodeId) -> Option<Vec<usize>> {
    if let Some(index) = nodes.iter().position(|node| node.id_str() == id.as_str()) {
        return Some(vec![index]);
    }
    for (index, node) in nodes.iter().enumerate() {
        if let Some(tail) = node
            .children()
            .and_then(|children| remove_path(children, id))
        {
            let mut path = Vec::with_capacity(tail.len() + 1);
            path.push(index);
            path.extend(tail);
            return Some(path);
        }
    }
    None
}
pub(super) fn indexed_read_path(
    state: &EditorState,
    scope: &Scope,
    roots: &[PenNode],
    id: &NodeId,
) -> Option<Vec<usize>> {
    state
        .command_lookup_path(
            scope,
            id,
            crate::command_id_scope::sequence::LookupOrder::Preorder,
        )
        .unwrap_or_else(|| read_path(roots, id).map(Vec::into_boxed_slice))
        .map(Vec::from)
}
fn indexed_remove_path(
    state: &EditorState,
    scope: &Scope,
    roots: &[PenNode],
    id: &NodeId,
) -> Option<Vec<usize>> {
    state
        .command_lookup_path(
            scope,
            id,
            crate::command_id_scope::sequence::LookupOrder::SiblingFirst,
        )
        .unwrap_or_else(|| remove_path(roots, id).map(Vec::into_boxed_slice))
        .map(Vec::from)
}
pub(super) fn authored<'a>(roots: &'a [PenNode], path: &[usize]) -> Option<&'a PenNode> {
    let mut nodes = roots;
    let mut result = None;
    for index in path {
        let node = nodes.get(*index)?;
        if matches!(node, PenNode::Ref(_)) {
            return None;
        }
        result = Some(node);
        nodes = node.children().map_or(&[], Vec::as_slice);
    }
    result
}
#[path = "target_insert_plan.rs"]
mod rich;
pub(super) use rich::insertion_bound;
pub(super) fn plan(state: &EditorState, command: &C, active: usize) -> Option<TargetSpec> {
    if let Some(plan) = rich::plan(state, command, active) {
        return Some(plan);
    }
    let raw = match command {
        C::InsertNode { page_id, .. }
        | C::DeleteNode { page_id, .. }
        | C::CopyNode { page_id, .. } => page_id.as_deref(),
        _ => return None,
    };
    let page = command_scope::page_index(state, raw, active)?;
    let scope = command_scope::scope(state, page);
    let roots = match &scope {
        Scope::Root => &state.doc.children,
        Scope::Page { index, .. } => &state.doc.pages.as_ref()?[*index].children,
    };
    if let C::DeleteNode { node_id, .. } = command {
        let mut path = indexed_remove_path(state, &scope, roots, node_id)?;
        let source = authored(roots, &path)?;
        let mut forecast = structural_payload_bound(source)?;
        let index = path.pop()?;
        forecast = forecast.checked_add(destination_bound(state, &scope, &path, 1)?)?;
        if forecast > crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE {
            return None;
        }
        return Some(TargetSpec {
            scope,
            path: path.into_boxed_slice(),
            subtree: false,
            splice: Some(SpliceSpec {
                index,
                remove: 1,
                insert_limit: 0,
                forecast,
            }),
            topology: None,
        });
    }
    let mut forecast = 128 * 1024;
    let parent = match command {
        C::InsertNode {
            target_parent,
            kind,
            name,
            fill_hex,
            ..
        } => {
            if kind.len() > 1024
                || name.len() > 64 * 1024
                || fill_hex.as_ref().is_some_and(|hex| hex.len() > 1024)
            {
                return None;
            }
            forecast += name.len() * 32;
            target_parent
        }
        C::CopyNode {
            target_parent,
            node_id,
            overrides_json,
            ..
        } => {
            let source = authored(roots, &indexed_read_path(state, &scope, roots, node_id)?)?;
            forecast = structural_payload_bound(source)?;
            if let Some(raw) = overrides_json {
                if raw.len() > 64 * 1024 {
                    return None;
                }
                forecast += raw.len() * 32;
                let value: serde_json::Value = serde_json::from_str(raw).ok()?;
                let object = value.as_object()?;
                if ["children", "type", "id"]
                    .iter()
                    .any(|key| object.contains_key(*key))
                {
                    return None;
                }
            }
            target_parent
        }
        _ => return None,
    };
    let (path, index) = if !parent.is_real() {
        (Vec::new(), roots.len())
    } else {
        let path = indexed_read_path(state, &scope, roots, parent)?;
        // CopyNode ultimately inserts with the sibling-first tree helper.
        if matches!(command, C::CopyNode { .. })
            && indexed_remove_path(state, &scope, roots, parent)? != path
        {
            return None;
        }
        let node = authored(roots, &path)?;
        if !node.is_container() {
            return None;
        }
        let count = node.children().map_or(0, Vec::len);
        (path, count)
    };
    forecast = forecast.checked_add(destination_bound(state, &scope, &path, 1)?)?;
    if forecast > crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE {
        return None;
    }
    Some(TargetSpec {
        scope,
        path: path.into_boxed_slice(),
        subtree: false,
        splice: Some(SpliceSpec {
            index,
            remove: 0,
            insert_limit: 1,
            forecast,
        }),
        topology: None,
    })
}

/// Include sibling shell relocation and identity/anchor preparation before any
/// allocation, without cloning sibling payloads. Static batches charge it once.

#[derive(Clone, Copy)]
pub(super) struct DestinationBudget {
    len: usize,
    capacity: usize,
    identities: usize,
    pub empty_root: bool,
}
impl DestinationBudget {
    pub(super) fn bound(&self, extra: usize) -> Option<usize> {
        let needed = self.len.checked_add(extra)?;
        let mut bytes = needed.checked_mul(256)?.checked_add(self.identities)?;
        if needed > self.capacity {
            bytes = bytes.checked_add(
                needed
                    .max(4)
                    .checked_mul(2)?
                    .checked_mul(std::mem::size_of::<PenNode>())?,
            )?;
        }
        (bytes <= crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE).then_some(bytes)
    }
}
pub(super) fn destination_shape(
    state: &EditorState,
    scope: &Scope,
    path: &[usize],
) -> Option<DestinationBudget> {
    let roots = match scope {
        Scope::Root => &state.doc.children,
        Scope::Page { index, .. } => &state.doc.pages.as_ref()?.get(*index)?.children,
    };
    let (nodes, capacity) = if path.is_empty() {
        (roots.as_slice(), roots.capacity())
    } else {
        let parent = authored(roots, path)?;
        parent.children().map_or((&[][..], 0), |children| {
            (children.as_slice(), children.capacity())
        })
    };
    let identities = nodes.iter().try_fold(0usize, |sum, node| {
        sum.checked_add(node.id_str().len().checked_mul(32)?)
    })?;
    let empty_root = path.is_empty()
        && nodes.iter().any(|node| {
            matches!(node, PenNode::Frame(_)) && node.children().is_none_or(Vec::is_empty)
        });
    Some(DestinationBudget {
        len: nodes.len(),
        capacity,
        identities,
        empty_root,
    })
}
pub(super) fn destination_bound(
    state: &EditorState,
    scope: &Scope,
    path: &[usize],
    extra: usize,
) -> Option<usize> {
    destination_shape(state, scope, path)?.bound(extra)
}
pub(super) fn append_destination(
    state: &EditorState,
    scope: &Scope,
    parent: &NodeId,
) -> Option<TargetSpec> {
    let roots = match scope {
        Scope::Root => &state.doc.children,
        Scope::Page { index, .. } => &state.doc.pages.as_ref()?.get(*index)?.children,
    };
    let (path, index) = if parent.is_real() {
        let path = indexed_read_path(state, &scope, roots, parent)?;
        if indexed_remove_path(state, &scope, roots, parent)? != path {
            return None;
        }
        let node = authored(roots, &path)?;
        if !node.is_container() {
            return None;
        }
        (path, node.children().map_or(0, Vec::len))
    } else {
        (Vec::new(), roots.len())
    };
    let forecast = 64 * 1024 + destination_bound(state, scope, &path, 1)?;
    Some(TargetSpec {
        scope: scope.clone(),
        path: path.into_boxed_slice(),
        subtree: false,
        splice: Some(SpliceSpec {
            index,
            remove: 0,
            insert_limit: 1,
            forecast,
        }),
        topology: None,
    })
}
