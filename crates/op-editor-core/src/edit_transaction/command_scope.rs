//! Audited forest/header classification; unknown global operations stay Full.
use crate::{history::Scope, EditorCommand as C, EditorState};
pub(super) fn scope(state: &EditorState, index: usize) -> Scope {
    match state.doc.pages.as_ref().filter(|pages| !pages.is_empty()) {
        Some(pages) => {
            let index = index.min(pages.len() - 1);
            Scope::Page {
                index,
                id: pages[index].id.clone(),
            }
        }
        None => Scope::Root,
    }
}
pub(super) fn page_index(state: &EditorState, raw: Option<&str>, active: usize) -> Option<usize> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Some(active.min(state.page_count().saturating_sub(1)));
    };
    match state.doc.pages.as_ref().filter(|pages| !pages.is_empty()) {
        Some(pages) => pages.iter().position(|p| p.id == raw).or_else(|| {
            raw.parse::<usize>()
                .ok()
                .filter(|index| *index < pages.len())
        }),
        None => raw.parse::<usize>().ok().filter(|index| *index == 0),
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScopedPlan {
    pub forests: Vec<Scope>,
    pub header: bool,
}
pub(super) enum Classification {
    Full,
    Refused,
    Scoped(ScopedPlan),
}
#[derive(Debug)]
enum PlanError {
    Full,
    Refused,
}
pub(super) fn classify(state: &EditorState, command: &C) -> Classification {
    if refused_header(state, command)
        || matches!(command, C::Undo) && state.history.past.is_empty()
        || matches!(command, C::Redo) && state.history.future.is_empty()
        || matches!(command, C::Batch { commands } if commands.is_empty())
    {
        return Classification::Refused;
    }
    let requested_page = match command {
        C::InsertNode { page_id, .. }
        | C::UpdateNode { page_id, .. }
        | C::PatchNodeData { page_id, .. }
        | C::DeleteNode { page_id, .. }
        | C::MoveNode { page_id, .. }
        | C::CopyNode { page_id, .. }
        | C::ReplaceNode { page_id, .. }
        | C::ReplaceSubtree { page_id, .. }
        | C::BatchInsert { page_id, .. }
        | C::InsertSubtree { page_id, .. }
        | C::InsertAuthoredSubtree { page_id, .. }
        | C::InsertAuthoredSubtreePreservingRoots { page_id, .. }
        | C::ImportSvg { page_id, .. }
        | C::RefineDesign { page_id, .. } => Some(page_id.as_deref()),
        _ => None,
    };
    if requested_page
        .is_some_and(|page| page_index(state, page, state.ui.active_page_index).is_none())
        || matches!(command, C::SetActivePage { index } if *index as usize >= state.page_count())
    {
        return Classification::Refused;
    }
    if state.editor_ui.workspace.active || state.undo_capture_suspended {
        return Classification::Full;
    }
    if let C::Undo | C::Redo = command {
        let entry = if matches!(command, C::Undo) {
            state.history.past.back()
        } else {
            state.history.future.back()
        }
        .unwrap();
        return entry
            .scoped_snapshot()
            .map_or(Classification::Full, |snapshot| {
                Classification::Scoped(ScopedPlan {
                    forests: snapshot.scopes(),
                    header: snapshot.captures_header(),
                })
            });
    }
    let mut out = ScopedPlan {
        forests: vec![scope(state, state.ui.active_page_index)],
        header: false,
    };
    let mut active = state.ui.active_page_index;
    match plan(state, command, &mut active, &mut out) {
        Ok(()) => Classification::Scoped(out),
        Err(PlanError::Refused) => Classification::Refused,
        Err(PlanError::Full) => Classification::Full,
    }
}

fn plan(
    state: &EditorState,
    command: &C,
    active: &mut usize,
    out: &mut ScopedPlan,
) -> Result<(), PlanError> {
    if let C::Batch { commands } = command {
        for command in commands {
            // A refused child mutates nothing. Other valid child scopes still
            // cover the exact prefix that the atomic Batch will roll back.
            match plan(state, command, active, out) {
                Ok(()) | Err(PlanError::Refused) => {}
                Err(error) => return Err(error),
            }
        }
        return Ok(());
    }
    let requested = match command {
        C::InsertNode { page_id, .. }
        | C::UpdateNode { page_id, .. }
        | C::PatchNodeData { page_id, .. }
        | C::DeleteNode { page_id, .. }
        | C::MoveNode { page_id, .. }
        | C::CopyNode { page_id, .. }
        | C::ReplaceNode { page_id, .. }
        | C::ReplaceSubtree { page_id, .. }
        | C::BatchInsert { page_id, .. }
        | C::InsertSubtree { page_id, .. }
        | C::InsertAuthoredSubtree { page_id, .. }
        | C::InsertAuthoredSubtreePreservingRoots { page_id, .. }
        | C::ImportSvg { page_id, .. } => {
            page_index(state, page_id.as_deref(), *active).ok_or(PlanError::Refused)?
        }
        C::DuplicateSelected { .. }
        | C::DeleteSelected
        | C::NudgeSelected { .. }
        | C::GroupSelected
        | C::ReplacePathsWithSvg(_)
        | C::UngroupSelected
        | C::ReorderSelected { .. }
        | C::SetNodeFlag { .. }
        | C::SetNodeFlip { .. }
        | C::SetEllipseArc { .. }
        | C::AddNodeEffect { .. }
        | C::RemoveNodeEffect { .. }
        | C::SetEffectParam { .. }
        | C::SetEffectColor { .. }
        | C::SetNodeRotation { .. }
        | C::SetNodeText { .. }
        | C::SetNodeCornerRadius { .. }
        | C::SetNodeFontSize { .. }
        | C::SetNodeFontWeight { .. }
        | C::SetNodeStrokeHex { .. }
        | C::SetNodeStrokeWidth { .. }
        | C::SetNodeStrokeSideWidth { .. }
        | C::SetNodeFillHex { .. }
        | C::SetNodeName { .. }
        | C::SetNodeLayoutProp { .. }
        | C::AlignSelected { .. }
        | C::CutSelected
        | C::PasteClipboard { .. }
        | C::SetImageVideoSrc { .. }
        | C::SetImageVideoPlayback { .. }
        | C::AddImageVideo { .. }
        | C::RemoveImageVideo { .. } => *active,
        C::CreateComponent { .. } | C::InstantiateComponent { .. } | C::RenameComponent { .. } => {
            *active
        }
        C::DeleteComponent { component_id } => {
            // Include every pre-existing match: an earlier Batch command may
            // remove the first duplicate and expose a later page/root match.
            if let Some(pages) = &state.doc.pages {
                for (index, page) in pages.iter().enumerate() {
                    if crate::walkers::find_node(&page.children, component_id).is_some() {
                        let target = scope(state, index);
                        if !out.forests.contains(&target) {
                            out.forests.push(target);
                        }
                    }
                }
            }
            if crate::walkers::find_node(&state.doc.children, component_id).is_some()
                && !out.forests.contains(&Scope::Root)
            {
                out.forests.push(Scope::Root);
            }
            *active
        }
        C::SetVariableColor { .. }
        | C::SetVariableScalar { .. }
        | C::CreateVariable { .. }
        | C::SetVariables { .. }
        | C::SetThemes { .. }
        | C::MergeThemePreset { .. }
        | C::SetDesignMd { .. }
        | C::MergeAppState { .. }
        | C::UpsertVariables { .. } => {
            out.header = true;
            *active
        }
        C::DeleteVariable { name } | C::RenameVariable { old_name: name, .. } => {
            out.header = true;
            let token = format!("${name}");
            if let Some(pages) = &state.doc.pages {
                for (index, page) in pages.iter().enumerate() {
                    if crate::history::contains_header_token(&page.children, &token) {
                        let target = scope(state, index);
                        if !out.forests.contains(&target) {
                            out.forests.push(target);
                        }
                    }
                }
            }
            if crate::history::contains_header_token(&state.doc.children, &token)
                && !out.forests.contains(&Scope::Root)
            {
                out.forests.push(Scope::Root);
            }
            *active
        }
        C::ClearSelection
        | C::SetSelection { .. }
        | C::SetSelectionSet { .. }
        | C::ToggleNodeSelection { .. } => *active,
        C::SetActivePage { index } => {
            let index = *index as usize;
            if index >= state.page_count() {
                return Err(PlanError::Refused);
            }
            *active = index;
            index
        }
        // Unclassified document fields, page shape, arbitrary callbacks, and
        // future commands retain complete snapshots.
        _ => return Err(PlanError::Full),
    };
    let target = scope(state, requested);
    if !out.forests.contains(&target) {
        out.forests.push(target);
    }
    Ok(())
}

// Only read-only conditions that guarantee the current command returns false.
// This is checked for the outer command, never against a Batch's future state.
fn refused_header(state: &EditorState, command: &C) -> bool {
    use jian_ops_schema::variable::VariableKind;
    match command {
        C::SetVariableColor { name, hex } => {
            crate::color_picker::parse_hex_rgb(hex).is_none()
                || !state
                    .find_variable(name)
                    .is_some_and(|def| def.kind == VariableKind::Color)
        }
        C::SetVariableScalar { name, scalar } => !state.find_variable(name).is_some_and(|def| {
            matches!(
                (&def.kind, scalar),
                (
                    VariableKind::Number,
                    crate::VariableScalarPayload::Number(_)
                ) | (
                    VariableKind::String,
                    crate::VariableScalarPayload::String(_)
                ) | (
                    VariableKind::Boolean,
                    crate::VariableScalarPayload::Boolean(_)
                )
            )
        }),
        C::CreateVariable { name, .. } => {
            name.trim().is_empty() || state.find_variable(name.trim()).is_some()
        }
        C::DeleteVariable { name } => state.find_variable(name).is_none(),
        C::RenameVariable { old_name, new_name } => {
            state.find_variable(old_name).is_none()
                || new_name.trim().is_empty()
                || (old_name != new_name.trim() && state.find_variable(new_name.trim()).is_some())
        }
        C::UpsertVariables { variables, .. } => variables.is_empty(),
        _ => false,
    }
}
