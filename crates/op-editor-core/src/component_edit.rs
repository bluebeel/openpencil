//! Resolve component registration once, preserving the legacy first document
//! identity even when an active page contains another node with the same ID.
use super::{
    find_node_and_location, is_component_root, set_reusable, shallow_root, Component, EditorState,
    NodeId, PenNode, PenNodeExt,
};
use crate::component_backing::DocumentNodeLocation;

struct Plan {
    active: Box<[usize]>,
    location: DocumentNodeLocation,
    root: PenNode,
    name: String,
}
fn plan(state: &EditorState, id: &NodeId, name: Option<&str>) -> Option<Plan> {
    if !id.is_real() || name.is_some_and(|name| name.trim().is_empty()) {
        return None;
    }
    let (global, location) = find_node_and_location(&state.doc, id.as_str())?;
    let active_page = state
        .doc
        .pages
        .as_ref()
        .filter(|pages| !pages.is_empty())
        .map(|pages| state.ui.active_page_index.min(pages.len() - 1));
    let same_page = location.page_index_in(&state.doc)? == active_page;
    let active = if same_page {
        location.child_path.clone()
    } else {
        super::routes::find(state.active_children(), id.as_str())?
    };
    let source = super::routes::at(state.active_children(), &active)?;
    if !is_component_root(source) {
        return None;
    }
    let name = name.map(str::to_owned).unwrap_or_else(|| {
        source
            .base()
            .name
            .clone()
            .unwrap_or_else(|| source.id_str().to_owned())
    });
    let name = name.trim().to_owned();
    if name.is_empty() {
        return None;
    }
    let mut root = shallow_root(global);
    if same_page {
        set_reusable(&mut root, true);
    }
    Some(Plan {
        active,
        location,
        root,
        name,
    })
}
pub(super) fn create(state: &mut EditorState, id: &NodeId, name: Option<&str>) -> bool {
    let Some(plan) = plan(state, id, name) else {
        return false;
    };
    let pending = crate::history::PendingScopedSnapshot::capture_auxiliary(
        state,
        &crate::EditorCommand::CreateComponent {
            node_id: id.clone(),
            name: plan.name.clone(),
        },
    );
    let snapshot = pending
        .is_none()
        .then(|| state.snapshot_for_undo())
        .flatten();
    set_reusable(
        super::routes::at_mut(state.active_children_mut(), &plan.active)
            .expect("checked active component route"),
        true,
    );
    state.components.insert_document_backed(
        Component {
            id: id.clone(),
            name: plan.name,
            root: plan.root,
        },
        plan.location,
    );
    state.finish_component_history(pending, snapshot);
    true
}
