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
fn authored<'a>(roots: &'a [PenNode], path: &[usize]) -> Option<&'a PenNode> {
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
pub(super) fn plan(state: &EditorState, command: &C, active: usize) -> Option<TargetSpec> {
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
        let mut path = remove_path(roots, node_id)?;
        let source = authored(roots, &path)?;
        let forecast = structural_payload_bound(source)?;
        let index = path.pop()?;
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
            let source = authored(roots, &read_path(roots, node_id)?)?;
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
        let path = read_path(roots, parent)?;
        // CopyNode ultimately inserts with the sibling-first tree helper.
        if matches!(command, C::CopyNode { .. }) && remove_path(roots, parent)? != path {
            return None;
        }
        let node = authored(roots, &path)?;
        if !node.is_container() {
            return None;
        }
        let count = node.children().map_or(0, Vec::len);
        (path, count)
    };
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
