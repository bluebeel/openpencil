//! Conservative target eligibility. Structural/dynamic batches keep page scopes.
use super::command_scope;
use crate::{
    history::{Scope, TargetSpec},
    EditorCommand as C, EditorState, NodeId, PenNodeExt,
};
use jian_ops_schema::node::PenNode;

pub(super) fn plan(state: &EditorState, command: &C) -> Option<Vec<TargetSpec>> {
    if state.instance_write_virtual_anchor.is_some() {
        return None;
    }
    let mut out = Vec::new();
    let mut active = state.ui.active_page_index;
    collect(state, command, &mut active, &mut out, false)?;
    Some(out)
}
fn collect(
    state: &EditorState,
    command: &C,
    active: &mut usize,
    out: &mut Vec<TargetSpec>,
    in_batch: bool,
) -> Option<()> {
    let mut subtree = false;
    let (id, page) = match command {
        C::Batch { commands } => {
            for command in commands {
                collect(state, command, active, out, true)?;
            }
            return Some(());
        }
        C::UpdateNode {
            node_id, page_id, ..
        } => (node_id, page_id.as_deref()),
        C::PatchNodeData {
            node_id,
            page_id,
            patch_json,
        } => {
            let patch: serde_json::Value = serde_json::from_str(patch_json).ok()?;
            let object = patch.as_object()?;
            if ["children", "type", "id"]
                .iter()
                .any(|key| object.contains_key(*key))
            {
                return None;
            }
            (node_id, page_id.as_deref())
        }
        C::ReplaceNode {
            node_id, page_id, ..
        }
        | C::ReplaceSubtree {
            node_id, page_id, ..
        } if !in_batch => {
            subtree = true;
            (node_id, page_id.as_deref())
        }
        C::SetNodeFlag { node_id, .. }
        | C::SetNodeFlip { node_id, .. }
        | C::SetEllipseArc { node_id, .. }
        | C::AddNodeEffect { node_id, .. }
        | C::RemoveNodeEffect { node_id, .. }
        | C::SetEffectParam { node_id, .. }
        | C::SetEffectColor { node_id, .. }
        | C::SetNodeRotation { node_id, .. }
        | C::SetNodeText { node_id, .. }
        | C::SetNodeCornerRadius { node_id, .. }
        | C::SetNodeFontSize { node_id, .. }
        | C::SetNodeFontWeight { node_id, .. }
        | C::SetNodeStrokeHex { node_id, .. }
        | C::SetNodeStrokeWidth { node_id, .. }
        | C::SetNodeStrokeSideWidth { node_id, .. }
        | C::SetNodeFillHex { node_id, .. }
        | C::SetNodeName { node_id, .. }
        | C::SetNodeLayoutProp { node_id, .. } => (node_id, None),
        C::SetVariableColor { .. }
        | C::SetVariableScalar { .. }
        | C::CreateVariable { .. }
        | C::SetVariables { .. }
        | C::SetThemes { .. }
        | C::MergeThemePreset { .. }
        | C::SetDesignMd { .. }
        | C::MergeAppState { .. }
        | C::UpsertVariables { .. }
        | C::ClearSelection
        | C::SetSelection { .. }
        | C::SetSelectionSet { .. }
        | C::ToggleNodeSelection { .. } => return Some(()),
        C::SetActivePage { index } => {
            *active = *index as usize;
            return Some(());
        }
        _ => return None,
    };
    let index = command_scope::page_index(state, page, *active)?;
    let scope = command_scope::scope(state, index);
    let nodes = match &scope {
        Scope::Root => &state.doc.children,
        Scope::Page { index, .. } => &state.doc.pages.as_ref()?[*index].children,
    };
    let path = find_path(nodes, id, &mut Vec::new())?;
    let mut node = nodes.get(path[0])?;
    if matches!(node, PenNode::Ref(_)) {
        return None;
    }
    for index in &path[1..] {
        node = node.children()?.get(*index)?;
        if matches!(node, PenNode::Ref(_)) {
            return None;
        }
    }
    let spec = TargetSpec {
        scope,
        path: path.into_boxed_slice(),
        subtree,
    };
    if !out.contains(&spec) {
        out.push(spec);
    }
    Some(())
}
fn find_path(nodes: &[PenNode], id: &NodeId, path: &mut Vec<usize>) -> Option<Vec<usize>> {
    // Exactly the command walker's immediate-sibling-before-descendant order.
    if let Some(index) = nodes.iter().position(|node| node.id_str() == id.as_str()) {
        let mut found = path.clone();
        found.push(index);
        return Some(found);
    }
    for (index, node) in nodes.iter().enumerate() {
        if let Some(children) = node.children() {
            path.push(index);
            let found = find_path(children, id, path);
            path.pop();
            if found.is_some() {
                return found;
            }
        }
    }
    None
}
