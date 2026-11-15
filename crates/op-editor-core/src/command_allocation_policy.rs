//! Standalone id seeding is independent from document dirtiness.
//!
//! This exhaustive match forces new commands to declare their allocation
//! behavior. Paths that mint ids keep the original pre-command seed; edits
//! with authored or existing ids do not require an unused allocator counter.

use crate::EditorCommand as C;

pub(crate) fn requires_id_seed(command: &C) -> bool {
    match command {
        C::Batch { commands } => commands.iter().any(requires_id_seed),
        C::InsertNode { .. }
        | C::CopyNode { .. }
        | C::ReplaceNode { .. }
        | C::ReplaceSubtree { .. }
        | C::BatchInsert { .. }
        | C::InsertSubtree { .. }
        | C::AdoptSceneTemplate { .. }
        | C::RefineDesign { .. }
        | C::UpsertComponent { .. }
        | C::UpsertScreen { .. }
        | C::InstantiateComponent { .. }
        | C::DetachComponent { .. }
        | C::InstantiateKitComponent { .. }
        | C::AddPage { .. }
        | C::DuplicatePage { .. }
        | C::DuplicateSelected { .. }
        | C::GroupSelected
        | C::ReplacePathsWithSvg(_)
        | C::PasteClipboard { .. }
        | C::ImportSvg { .. }
        | C::PromoteLegacyWidgets => true,
        C::SetInstanceComponent { .. }
        | C::CreateComponent { .. }
        | C::SetVariableColor { .. }
        | C::InsertAuthoredSubtree { .. }
        | C::InsertAuthoredSubtreePreservingRoots { .. }
        | C::SetActiveAxisValue { .. }
        | C::CycleActiveAxisValue { .. }
        | C::UpdateNode { .. }
        | C::PatchNodeData { .. }
        | C::DeleteNode { .. }
        | C::MoveNode { .. }
        | C::SetVariableScalar { .. }
        | C::CreateVariable { .. }
        | C::DeleteVariable { .. }
        | C::RenameVariable { .. }
        | C::SetVariables { .. }
        | C::UpsertVariables { .. }
        | C::SetThemes { .. }
        | C::MergeThemePreset { .. }
        | C::MergeAppState { .. }
        | C::SetDesignMd { .. }
        | C::DeleteComponent { .. }
        | C::RenameComponent { .. }
        | C::SetActivePage { .. }
        | C::RenamePage { .. }
        | C::DeletePage { .. }
        | C::ReorderPage { .. }
        | C::ClearSelection
        | C::SetSelection { .. }
        | C::SetSelectionSet { .. }
        | C::ToggleNodeSelection { .. }
        | C::SetViewport { .. }
        | C::SetNodeFlag { .. }
        | C::SetNodeFlip { .. }
        | C::SetEllipseArc { .. }
        | C::AddNodeEffect { .. }
        | C::RemoveNodeEffect { .. }
        | C::SetEffectParam { .. }
        | C::SetEffectColor { .. }
        | C::SetActiveTool { .. }
        | C::Undo
        | C::Redo
        | C::DeleteSelected
        | C::NudgeSelected { .. }
        | C::UngroupSelected
        | C::ReorderSelected { .. }
        | C::SetNodeRotation { .. }
        | C::SetNodeText { .. }
        | C::SetNodeCornerRadius { .. }
        | C::SetNodeFontSize { .. }
        | C::SetNodeFontWeight { .. }
        | C::SetNodeStrokeHex { .. }
        | C::SetNodeStrokeWidth { .. }
        | C::SetNodeStrokeSideWidth { .. }
        | C::AlignSelected { .. }
        | C::SetNodeFillHex { .. }
        | C::SetNodeName { .. }
        | C::SetImageVideoSrc { .. }
        | C::SetImageVideoPlayback { .. }
        | C::AddImageVideo { .. }
        | C::RemoveImageVideo { .. }
        | C::CopySelected
        | C::CutSelected
        | C::SetNodeLayoutProp { .. }
        | C::ReplaceFontFamily { .. }
        | C::ReplaceAllMatchingProperties { .. } => false,
    }
}

/// A direct reusable-definition detach allocates nothing. Keep this path
/// usable at counter exhaustion; Ref detachment still uses the caller's exact
/// normal seed. Mixed Batch conservatively retains its existing static policy.
pub(crate) fn requires_id_seed_in(state: &crate::EditorState, command: &C) -> bool {
    if let C::DetachComponent { node_id } = command {
        fn first<'a>(
            nodes: &'a [jian_ops_schema::node::PenNode],
            id: &str,
        ) -> Option<&'a jian_ops_schema::node::PenNode> {
            use crate::PenNodeExt;
            for node in nodes {
                if node.id_str() == id {
                    return Some(node);
                }
                if let Some(found) = node.children().and_then(|children| first(children, id)) {
                    return Some(found);
                }
            }
            None
        }
        return matches!(
            first(state.active_children(), node_id.as_str()),
            Some(jian_ops_schema::node::PenNode::Ref(_))
        );
    }
    requires_id_seed(command)
}

/// Changes to page/node presence need identity tracking; scalar and UI edits do not.
pub(crate) fn may_change_identities(command: &C) -> bool {
    match command {
        C::Batch { commands } => commands.iter().any(may_change_identities),
        C::InsertNode { .. }
        | C::CopyNode { .. }
        | C::ReplaceNode { .. }
        | C::ReplaceSubtree { .. }
        | C::BatchInsert { .. }
        | C::InsertSubtree { .. }
        | C::AdoptSceneTemplate { .. }
        | C::RefineDesign { .. }
        | C::UpsertComponent { .. }
        | C::UpsertScreen { .. }
        | C::InstantiateComponent { .. }
        | C::DetachComponent { .. }
        | C::InstantiateKitComponent { .. }
        | C::AddPage { .. }
        | C::DuplicatePage { .. }
        | C::DuplicateSelected { .. }
        | C::GroupSelected
        | C::ReplacePathsWithSvg(_)
        | C::PasteClipboard { .. }
        | C::ImportSvg { .. }
        | C::PromoteLegacyWidgets
        | C::InsertAuthoredSubtree { .. }
        | C::InsertAuthoredSubtreePreservingRoots { .. }
        | C::PatchNodeData { .. }
        | C::DeleteNode { .. }
        | C::DeletePage { .. }
        | C::Undo
        | C::Redo
        | C::DeleteSelected
        | C::UngroupSelected
        | C::CutSelected => true,
        C::SetInstanceComponent { .. }
        | C::CreateComponent { .. }
        | C::SetVariableColor { .. }
        | C::SetActiveAxisValue { .. }
        | C::CycleActiveAxisValue { .. }
        | C::UpdateNode { .. }
        | C::MoveNode { .. }
        | C::SetVariableScalar { .. }
        | C::CreateVariable { .. }
        | C::DeleteVariable { .. }
        | C::RenameVariable { .. }
        | C::SetVariables { .. }
        | C::UpsertVariables { .. }
        | C::SetThemes { .. }
        | C::MergeThemePreset { .. }
        | C::MergeAppState { .. }
        | C::SetDesignMd { .. }
        | C::DeleteComponent { .. }
        | C::RenameComponent { .. }
        | C::SetActivePage { .. }
        | C::RenamePage { .. }
        | C::ReorderPage { .. }
        | C::ClearSelection
        | C::SetSelection { .. }
        | C::SetSelectionSet { .. }
        | C::ToggleNodeSelection { .. }
        | C::SetViewport { .. }
        | C::SetNodeFlag { .. }
        | C::SetNodeFlip { .. }
        | C::SetEllipseArc { .. }
        | C::AddNodeEffect { .. }
        | C::RemoveNodeEffect { .. }
        | C::SetEffectParam { .. }
        | C::SetEffectColor { .. }
        | C::SetActiveTool { .. }
        | C::NudgeSelected { .. }
        | C::ReorderSelected { .. }
        | C::SetNodeRotation { .. }
        | C::SetNodeText { .. }
        | C::SetNodeCornerRadius { .. }
        | C::SetNodeFontSize { .. }
        | C::SetNodeFontWeight { .. }
        | C::SetNodeStrokeHex { .. }
        | C::SetNodeStrokeWidth { .. }
        | C::SetNodeStrokeSideWidth { .. }
        | C::AlignSelected { .. }
        | C::SetNodeFillHex { .. }
        | C::SetNodeName { .. }
        | C::SetImageVideoSrc { .. }
        | C::SetImageVideoPlayback { .. }
        | C::AddImageVideo { .. }
        | C::RemoveImageVideo { .. }
        | C::CopySelected
        | C::SetNodeLayoutProp { .. }
        | C::ReplaceFontFamily { .. }
        | C::ReplaceAllMatchingProperties { .. } => false,
    }
}
