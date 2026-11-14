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
        // build_leaf_node produces no registered/reusable definition; its ID
        // is fresh across the complete document. A document-backed parent
        // resolves changed children from canonical Doc, not its catalog header.
        C::InsertNode { .. } => splice.remove == 0 && splice.insert_limit == 1,
        C::DeleteNode { .. } => {
            let mut nodes = match &spec.scope {
                Scope::Root => state.doc.children.as_slice(),
                Scope::Page { index, id } => {
                    let Some(page) = state.doc.pages.as_ref().and_then(|pages| pages.get(*index))
                    else {
                        return false;
                    };
                    if page.id != *id {
                        return false;
                    }
                    page.children.as_slice()
                }
            };
            for index in spec.path.iter() {
                let Some(node) = nodes.get(*index) else {
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
            let registered: HashSet<_> = state
                .components
                .components
                .iter()
                .map(|component| component.id.as_str())
                .collect();
            let mut stack = vec![removed.iter()];
            while let Some(siblings) = stack.last_mut() {
                if let Some(node) = siblings.next() {
                    if registered.contains(node.id_str())
                        || matches!(node, PenNode::Frame(frame) if frame.reusable == Some(true))
                    {
                        return false;
                    }
                    if let Some(children) = node.children() {
                        stack.push(children.iter());
                    }
                } else {
                    stack.pop();
                }
            }
            true
        }
        _ => false,
    }
}
