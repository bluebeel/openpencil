//! Rich canonical insertions use the same exact bounded splice inverse as
//! legacy leaves. Original fields, children presence and allocation rules stay
//! in the real command; this planner borrows only its affected payload.
use super::{authored, command_scope, indexed_read_path};
use crate::{
    history::{structural_payload_bound, Scope, SpliceSpec, TargetSpec},
    EditorCommand as C, EditorState, PenNodeExt,
};
use jian_ops_schema::node::PenNode;
const LIMIT: usize = crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE;
fn add(bytes: &mut usize, n: usize) -> Option<()> {
    *bytes = bytes.checked_add(n)?;
    (*bytes <= LIMIT).then_some(())
}

pub(super) fn plan(state: &EditorState, command: &C, active: usize) -> Option<TargetSpec> {
    let (raw, parent, nodes, count, replace) = match command {
        C::InsertSubtree {
            nodes,
            parent_id,
            page_id,
        }
        | C::InsertAuthoredSubtree {
            nodes,
            parent_id,
            page_id,
        } => (
            page_id.as_deref(),
            Some(parent_id),
            Some(nodes),
            nodes.len(),
            true,
        ),
        C::InsertAuthoredSubtreePreservingRoots {
            nodes,
            parent_id,
            page_id,
        } => (
            page_id.as_deref(),
            Some(parent_id),
            Some(nodes),
            nodes.len(),
            false,
        ),
        C::BatchInsert { items, page_id } => (page_id.as_deref(), None, None, items.len(), false),
        _ => return None,
    };
    if count == 0 || count > 4096 {
        return None;
    }
    let page = command_scope::page_index(state, raw, active)?;
    let scope = command_scope::scope(state, page);
    let roots = match &scope {
        Scope::Root => &state.doc.children,
        Scope::Page { index, .. } => &state.doc.pages.as_ref()?.get(*index)?.children,
    };
    let mut forecast = insertion_bound(command)?;
    let (path, mut index) = match parent.filter(|parent| parent.is_real()) {
        Some(parent) => {
            let path = indexed_read_path(state, &scope, roots, parent)?;
            let node = authored(roots, &path)?;
            if !node.is_container() {
                return None;
            }
            (path, node.children().map_or(0, Vec::len))
        }
        None => (Vec::new(), roots.len()),
    };
    let mut remove = 0;
    if path.is_empty()
        && replace
        && nodes.is_some_and(|nodes| nodes.len() == 1 && matches!(nodes[0], PenNode::Frame(_)))
    {
        // The real replacement copies the first empty frame's coordinates,
        // removes its first matching root ID, then appends. Only a terminal
        // replacement is one contiguous splice. Nonterminal swaps retain the
        // ordinary path rather than pretending unrelated roots did not move.
        if let Some(empty) = roots.iter().find(|node| {
            matches!(node, PenNode::Frame(_)) && node.children().is_none_or(Vec::is_empty)
        }) {
            let removed = roots
                .iter()
                .position(|node| node.id_str() == empty.id_str())?;
            if removed.checked_add(1)? != roots.len() {
                return None;
            }
            index = removed;
            remove = 1;
            add(&mut forecast, structural_payload_bound(&roots[removed])?)?;
        }
    }
    add(
        &mut forecast,
        super::destination_bound(state, &scope, &path, count)?,
    )?;
    Some(TargetSpec {
        scope,
        path: path.into_boxed_slice(),
        subtree: false,
        splice: Some(SpliceSpec {
            index,
            remove,
            insert_limit: count,
            forecast,
        }),
        topology: None,
    })
}

pub(crate) fn insertion_bound(command: &C) -> Option<usize> {
    let mut forecast = 64 * 1024usize;
    if let C::InsertSubtree { nodes, .. }
    | C::InsertAuthoredSubtree { nodes, .. }
    | C::InsertAuthoredSubtreePreservingRoots { nodes, .. } = command
    {
        for node in nodes {
            add(&mut forecast, structural_payload_bound(node)?)?;
        }
    } else if let C::BatchInsert { items, .. } = command {
        for item in items {
            // The legacy leaf constructor has bounded defaults, but arbitrary
            // canonical fill stacks must be measured completely before cloning.
            add(&mut forecast, 128 * 1024)?;
            add(&mut forecast, item.name.len().checked_mul(32)?)?;
            add(&mut forecast, item.kind.len().checked_mul(8)?)?;
            add(
                &mut forecast,
                item.fill_hex
                    .as_ref()
                    .map_or(0, |fill| fill.len())
                    .checked_mul(8)?,
            )?;
            if let Some(fill) = &item.fill {
                let bytes = crate::history_snapshot::command_capture_value_bytes(
                    fill,
                    LIMIT.checked_sub(forecast)?,
                )
                .ok()?;
                add(&mut forecast, bytes)?;
            }
        }
    } else if let C::InsertNode {
        kind,
        name,
        fill_hex,
        ..
    } = command
    {
        if kind.len() > 1024
            || name.len() > 64 * 1024
            || fill_hex.as_ref().is_some_and(|value| value.len() > 1024)
        {
            return None;
        }
        add(&mut forecast, 128 * 1024)?;
        add(&mut forecast, name.len().checked_mul(32)?)?;
    } else {
        return None;
    }
    Some(forecast)
}
