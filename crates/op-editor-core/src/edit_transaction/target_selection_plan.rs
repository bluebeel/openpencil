//! One request-local pass for header-only selection operations. Unknown or
//! ambiguous canonical targets retain the ordinary compatibility path.
use super::{command_scope, Scope, TargetSpec};
use crate::{EditorCommand as C, EditorState, PenNodeExt};
use jian_ops_schema::node::PenNode;
use std::{
    collections::{HashMap, HashSet},
    mem,
};
const LIMIT: usize = crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE;
struct Frame<'a> {
    nodes: &'a [PenNode],
    next: usize,
    reference: bool,
}
pub(super) fn plan(
    state: &EditorState,
    command: &C,
    active: usize,
) -> Option<(Vec<TargetSpec>, usize)> {
    if !matches!(command, C::NudgeSelected { .. } | C::AlignSelected { .. })
        || state.editor_ui.workspace.active
        || state.instance_write_virtual_anchor.is_some()
    {
        return None;
    }
    let index = command_scope::page_index(state, None, active)?;
    let scope = command_scope::scope(state, index);
    let roots = match &scope {
        Scope::Root => state.doc.children.as_slice(),
        Scope::Page { index, .. } => state.doc.pages.as_ref()?.get(*index)?.children.as_slice(),
    };
    let wanted = state
        .selection
        .set
        .iter()
        .map(|id| id.as_str())
        .collect::<HashSet<_>>();
    let mut found = HashMap::with_capacity(wanted.len());
    let mut path = Vec::new();
    let mut bytes = 16 * 1024usize;
    let mut stack = vec![Frame {
        nodes: roots,
        next: 0,
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
        let reference = frame.reference || matches!(node, PenNode::Ref(_));
        if wanted.contains(node.id_str()) {
            if reference || found.contains_key(node.id_str()) {
                return None;
            }
            let metadata = crate::history_snapshot::command_capture_node_bytes(
                node,
                true,
                LIMIT.checked_sub(bytes)?,
            )
            .ok()?;
            bytes = bytes
                .checked_add(metadata)?
                .checked_add(path.len().checked_mul(mem::size_of::<usize>() * 4)?)?
                .checked_add(node.id_str().len().checked_mul(8)?)?;
            if bytes > LIMIT {
                return None;
            }
            found.insert(
                node.id_str(),
                TargetSpec {
                    scope: scope.clone(),
                    path: path.clone().into_boxed_slice(),
                    subtree: false,
                    splice: None,
                    topology: None,
                },
            );
        }
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            stack.push(Frame {
                nodes: children,
                next: 0,
                reference,
            });
        } else {
            path.pop();
        }
    }
    let mut specs = Vec::with_capacity(found.len());
    for id in &state.selection.set {
        if let Some(spec) = found.remove(id.as_str()) {
            specs.push(spec);
        }
    }
    Some((specs, bytes))
}
