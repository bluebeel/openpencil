//! Known page-local command classification; unknown/global operations stay Full.
use crate::{history::Scope, EditorCommand as C, EditorState};
fn scope(state: &EditorState, index: usize) -> Scope {
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
fn page_index(state: &EditorState, raw: Option<&str>, active: usize) -> Option<usize> {
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
pub(super) fn scopes(state: &EditorState, command: &C) -> Option<Vec<Scope>> {
    if state.editor_ui.workspace.active || state.undo_capture_suspended {
        return None;
    }
    if let C::Undo | C::Redo = command {
        let entry = if matches!(command, C::Undo) {
            state.history.past.back()?
        } else {
            state.history.future.back()?
        };
        return entry.scoped_snapshot().map(|snapshot| snapshot.scopes());
    }
    let mut out = vec![scope(state, state.ui.active_page_index)];
    let mut active = state.ui.active_page_index;
    plan(state, command, &mut active, &mut out)?;
    Some(out)
}
fn plan(state: &EditorState, command: &C, active: &mut usize, out: &mut Vec<Scope>) -> Option<()> {
    if let C::Batch { commands } = command {
        for command in commands {
            plan(state, command, active, out)?;
        }
        return Some(());
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
        | C::ImportSvg { page_id, .. } => page_index(state, page_id.as_deref(), *active)?,
        C::DuplicateSelected { .. }
        | C::DeleteSelected
        | C::NudgeSelected { .. }
        | C::GroupSelected
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
                        if !out.contains(&target) {
                            out.push(target);
                        }
                    }
                }
            }
            if crate::walkers::find_node(&state.doc.children, component_id).is_some()
                && !out.contains(&Scope::Root)
            {
                out.push(Scope::Root);
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
                return None;
            }
            *active = index;
            index
        }
        // Global document fields, page shape, component registry, arbitrary
        // callbacks, and future commands retain full snapshots.
        _ => return None,
    };
    let target = scope(state, requested);
    if !out.contains(&target) {
        out.push(target);
    }
    Some(())
}
