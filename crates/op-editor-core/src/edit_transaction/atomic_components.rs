//! Before-mutation proof for optional mixed-batch catalog checkpoints. Unknown
//! writers upgrade the original catalog before running; default APIs stay full.
use crate::{
    history::{Scope, TargetSpec},
    EditorCommand as C, EditorState, PenNodeExt,
};
use jian_ops_schema::node::PenNode;
use std::collections::HashSet;

pub(super) fn preserves(state: &EditorState, command: &C, targets: Option<&[TargetSpec]>) -> bool {
    if !command.may_change_document() || state.metadata_command_preserves_components(command) {
        return true;
    }
    if state.editor_ui.workspace.active
        || state.instance_write_virtual_anchor.is_some()
        || state
            .components
            .components
            .iter()
            .any(|component| !state.components.is_document_backed(&component.id))
    {
        return false;
    }
    if matches!(
        command,
        C::DuplicateSelected { .. } | C::PasteClipboard { .. }
    ) {
        return crate::mutators::clone_splice::catalog_preserving(state, command);
    }
    if matches!(command, C::DeleteSelected | C::CutSelected) {
        return crate::history::PendingScopedSnapshot::deletion_preserves_components(state);
    }
    let Some([spec]) = targets else {
        return false;
    };
    let Some(splice) = &spec.splice else {
        return false;
    };
    if spec.topology.is_some() {
        return false;
    }
    match command {
        // Factory leaves never introduce a reusable definition. Document-backed
        // parents resolve their current children from the complete canonical Doc.
        C::InsertNode { .. } | C::BatchInsert { .. } => splice.remove == 0,
        C::InsertSubtree { nodes, .. }
        | C::InsertAuthoredSubtree { nodes, .. }
        | C::InsertAuthoredSubtreePreservingRoots { nodes, .. } => {
            no_definitions(nodes) && removed_preserve_catalog(state, spec)
        }
        C::DeleteNode { .. } => removed_preserve_catalog(state, spec),
        _ => false,
    }
}
fn no_definitions(nodes: &[PenNode]) -> bool {
    let mut pending = vec![nodes.iter()];
    while let Some(siblings) = pending.last_mut() {
        if let Some(node) = siblings.next() {
            if matches!(node,PenNode::Frame(frame) if frame.reusable==Some(true)) {
                return false;
            }
            if let Some(children) = node.children() {
                pending.push(children.iter());
            }
        } else {
            pending.pop();
        }
    }
    true
}
fn removed_preserve_catalog(state: &EditorState, spec: &TargetSpec) -> bool {
    let Some(splice) = &spec.splice else {
        return false;
    };
    let mut nodes = match &spec.scope {
        Scope::Root => state.doc.children.as_slice(),
        Scope::Page { index, id } => {
            let Some(page) = state.doc.pages.as_ref().and_then(|pages| pages.get(*index)) else {
                return false;
            };
            if page.id != *id {
                return false;
            }
            page.children.as_slice()
        }
    };
    for at in spec.path.iter() {
        let Some(node) = nodes.get(*at) else {
            return false;
        };
        nodes = node.children().map_or(&[], Vec::as_slice);
    }
    let Some(end) = splice.index.checked_add(splice.remove) else {
        return false;
    };
    let Some(removed) = nodes.get(splice.index..end) else {
        return false;
    };
    if removed.is_empty() {
        return true;
    }
    let registered: HashSet<_> = state
        .components
        .components
        .iter()
        .map(|component| component.id.as_str())
        .collect();
    let mut pending = vec![removed.iter()];
    while let Some(siblings) = pending.last_mut() {
        if let Some(node) = siblings.next() {
            if registered.contains(node.id_str())
                || matches!(node,PenNode::Frame(frame) if frame.reusable==Some(true))
            {
                return false;
            }
            if let Some(children) = node.children() {
                pending.push(children.iter());
            }
        } else {
            pending.pop();
        }
    }
    true
}
