//! One exact operation allowlist for primary and resident durable cohorts.
//! Tool admission never replaces real registry validation or Core capture proof.
use op_editor_core::EditorCommand as C;
pub(crate) fn tool(name: &str) -> bool {
    matches!(
        name,
        "set_variable_color"
            | "set_variable_number"
            | "set_variable_string"
            | "set_variable_boolean"
            | "create_variable"
            | "rename_page"
            | "update_node"
            | "set_node_rotation"
            | "set_node_text"
            | "set_node_corner_radius"
            | "set_node_font_size"
            | "set_node_font_weight"
            | "set_node_stroke_hex"
            | "set_node_stroke_width"
            | "set_node_fill_hex"
            | "set_node_name"
            | "set_node_hidden"
            | "set_node_locked"
            | "set_node_collapsed"
            | "set_node_flip"
            | "set_ellipse_arc"
            | "add_node_effect"
            | "remove_node_effect"
            | "set_node_stroke_side_width"
            | "insert_node"
            | "delete_node"
            | "copy_node"
            | "duplicate_selected"
            | "paste_clipboard"
            | "move_node"
            | "reorder_selected"
            | "group_selected"
            | "ungroup_selected"
            | "delete_selected"
            | "cut_selected"
            | "boolean_selected"
            | "nudge_selected"
            | "align_selected"
            | "batch_design"
    )
}
pub(crate) fn structural_tool(name: &str) -> bool {
    matches!(
        name,
        "insert_node"
            | "delete_node"
            | "copy_node"
            | "duplicate_selected"
            | "paste_clipboard"
            | "move_node"
            | "reorder_selected"
            | "group_selected"
            | "ungroup_selected"
            | "delete_selected"
            | "cut_selected"
            | "boolean_selected"
            | "nudge_selected"
            | "align_selected"
            | "batch_design"
    )
}
pub(crate) fn command(command: &C) -> bool {
    let mut pending = vec![(command, 0usize)];
    let mut count = 0usize;
    while let Some((command, depth)) = pending.pop() {
        count += 1;
        if count > 4096 || depth > 32 {
            return false;
        }
        if let C::Batch { commands } = command {
            if commands.is_empty() {
                return false;
            }
            pending.extend(commands.iter().rev().map(|command| (command, depth + 1)));
            continue;
        }
        if !matches!(
            command,
            C::SetVariableColor { .. }
                | C::SetVariableScalar { .. }
                | C::CreateVariable { .. }
                | C::RenamePage { .. }
                | C::UpdateNode { .. }
                | C::PatchNodeData { .. }
                | C::SetNodeRotation { .. }
                | C::SetNodeText { .. }
                | C::SetNodeCornerRadius { .. }
                | C::SetNodeFontSize { .. }
                | C::SetNodeFontWeight { .. }
                | C::SetNodeStrokeHex { .. }
                | C::SetNodeStrokeWidth { .. }
                | C::SetNodeFillHex { .. }
                | C::SetNodeName { .. }
                | C::SetNodeFlag { .. }
                | C::SetNodeFlip { .. }
                | C::SetEllipseArc { .. }
                | C::AddNodeEffect { .. }
                | C::RemoveNodeEffect { .. }
                | C::SetEffectParam { .. }
                | C::SetEffectColor { .. }
                | C::SetNodeStrokeSideWidth { .. }
                | C::SetNodeLayoutProp { .. }
                | C::InsertNode { .. }
                | C::BatchInsert { .. }
                | C::InsertSubtree { .. }
                | C::InsertAuthoredSubtree { .. }
                | C::InsertAuthoredSubtreePreservingRoots { .. }
                | C::DeleteNode { .. }
                | C::CopyNode { .. }
                | C::DuplicateSelected { .. }
                | C::PasteClipboard { .. }
                | C::MoveNode { .. }
                | C::ReorderSelected { .. }
                | C::GroupSelected
                | C::UngroupSelected
                | C::DeleteSelected
                | C::CutSelected
                | C::ReplacePathsWithSvg(_)
                | C::NudgeSelected { .. }
                | C::AlignSelected { .. }
        ) {
            return false;
        }
    }
    true
}

/// Generated insertion/refine commands own a separate cleanup contract. Keep
/// that ordinary path unless the existing policy disables cleanup completely;
/// admitting a fast insert must never silently skip its semantic post-pass.
pub(crate) fn repair_free(state: &op_editor_core::EditorState, tool: &str, command: &C) -> bool {
    if !super::write_repairs::generated_tool(tool)
        || state.editor_ui.preserve_authored_geometry
        || state.editor_ui.workspace.active
    {
        return true;
    }
    let mut pending = vec![command];
    while let Some(command) = pending.pop() {
        match command {
            C::Batch { commands } => pending.extend(commands),
            C::InsertSubtree { .. }
            | C::InsertAuthoredSubtree { .. }
            | C::InsertAuthoredSubtreePreservingRoots { .. }
            | C::RefineDesign { .. } => return false,
            _ => {}
        }
    }
    true
}
