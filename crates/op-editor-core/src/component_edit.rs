//! Resolve component registration once, preserving the legacy first document
//! identity even when an active page contains another node with the same ID.
use super::{
    find_node_and_location, is_component_root, set_reusable, shallow_root, Component, EditorState,
    NodeId, PenNode, PenNodeExt,
};
use crate::component_backing::DocumentNodeLocation;

fn active_path(nodes: &[PenNode], id: &str, path: &mut Vec<usize>) -> Option<Box<[usize]>> {
    for (at, node) in nodes.iter().enumerate() {
        path.push(at);
        if node.id_str() == id {
            return Some(path.clone().into_boxed_slice());
        }
        if let Some(children) = node.children() {
            if let Some(found) = active_path(children, id, path) {
                return Some(found);
            }
        }
        path.pop();
    }
    None
}
fn node_at<'a>(mut nodes: &'a [PenNode], path: &[usize]) -> Option<&'a PenNode> {
    let (last, parents) = path.split_last()?;
    for at in parents {
        nodes = nodes.get(*at)?.children()?;
    }
    nodes.get(*last)
}
fn node_at_mut<'a>(mut nodes: &'a mut [PenNode], path: &[usize]) -> Option<&'a mut PenNode> {
    let (last, parents) = path.split_last()?;
    for at in parents {
        let node = nodes.get_mut(*at)?;
        if node.children().is_none() {
            return None;
        }
        nodes = node.children_mut()?;
    }
    nodes.get_mut(*last)
}
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
    let same_page = location.page_index == active_page;
    let active = if same_page {
        location.child_path.clone()
    } else {
        active_path(state.active_children(), id.as_str(), &mut Vec::new())?
    };
    let source = node_at(state.active_children(), &active)?;
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
    let snapshot = state.snapshot_for_undo();
    set_reusable(
        node_at_mut(state.active_children_mut(), &plan.active)
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
    state.push_undo(snapshot);
    true
}
