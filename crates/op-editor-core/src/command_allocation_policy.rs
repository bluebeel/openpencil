//! Standalone id seeding is independent from document dirtiness.
//!
//! This exhaustive match forces new commands to declare their allocation
//! behavior. Creation/refinement paths keep the original pre-command seed;
//! existing-node edits never scan ids or fail just because fresh ids ran out.

use crate::EditorCommand as C;

pub(super) fn requires_id_seed(command: &C) -> bool {
    match command {
        C::Batch { commands } => commands.iter().any(requires_id_seed),
        C::InsertNode { .. }
        | C::CopyNode { .. }
        | C::ReplaceNode { .. }
        | C::ReplaceSubtree { .. }
        | C::BatchInsert { .. }
        | C::InsertSubtree { .. }
        | C::InsertAuthoredSubtree { .. }
        | C::InsertAuthoredSubtreePreservingRoots { .. }
        | C::AdoptSceneTemplate { .. }
        | C::RefineDesign { .. }
        | C::UpsertComponent { .. }
        | C::UpsertScreen { .. }
        | C::InstantiateComponent { .. }
        | C::CreateComponent { .. }
        | C::InstantiateKitComponent { .. }
        | C::AddPage { .. }
        | C::DuplicatePage { .. }
        | C::DuplicateSelected { .. }
        | C::GroupSelected
        | C::PasteClipboard { .. }
        | C::ImportSvg { .. }
        | C::PromoteLegacyWidgets => true,
        C::SetVariableColor { .. }
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
