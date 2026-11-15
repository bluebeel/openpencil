//! Canonical component operations own one catalog entry and known node regions.
use super::super::{identity, patch, Scope, ScopedHistoryError, SpliceSpec, TargetSpec};
use super::catalog::{self, Entry};
use crate::{EditorCommand as C, EditorState, NodeId, PenNodeExt, SharedNode};
use jian_ops_schema::node::PenNode;
use std::sync::Arc;
const LIMIT: usize = crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE;
#[derive(Debug, Clone, PartialEq)]
enum Request {
    Create { id: NodeId, name: String },
    Delete(NodeId),
    Instantiate(NodeId),
    Detach,
}
#[derive(Debug)]
pub(crate) struct Pending {
    request: Request,
    id: NodeId,
    old: Option<Entry>,
    anchor: super::super::splice_order::Anchors,
    order: Vec<NodeId>,
    input: Option<Arc<SharedNode>>,
    catalog_delta: bool,
    check_entry: bool,
}
pub(super) struct Plan {
    pub pending: Pending,
    pub scopes: Vec<Scope>,
    pub targets: Vec<TargetSpec>,
}
pub(super) fn supports(command: &C) -> bool {
    matches!(
        command,
        C::CreateComponent { .. }
            | C::DeleteComponent { .. }
            | C::InstantiateComponent { .. }
            | C::DetachComponent { .. }
    )
}
fn eligible(state: &EditorState) -> bool {
    !state.undo_capture_suspended
        && !state.editor_ui.workspace.active
        && state.instance_write_virtual_anchor.is_none()
}
fn active(state: &EditorState) -> Scope {
    match state.doc.pages.as_ref().filter(|pages| !pages.is_empty()) {
        Some(pages) => {
            let index = state.ui.active_page_index.min(pages.len() - 1);
            Scope::Page {
                index,
                id: pages[index].id.clone(),
            }
        }
        None => Scope::Root,
    }
}
fn scope(
    location: &crate::component_backing::DocumentNodeLocation,
    state: &EditorState,
) -> Option<Scope> {
    match location.page_index_in(&state.doc)? {
        Some(index) => Some(Scope::Page {
            index,
            id: state.doc.pages.as_ref()?.get(index)?.id.clone(),
        }),
        None => Some(Scope::Root),
    }
}
fn header(scope: Scope, path: Box<[usize]>, subtree: bool) -> TargetSpec {
    TargetSpec {
        scope,
        path,
        subtree,
        splice: None,
        topology: None,
    }
}
fn bound(
    state: &EditorState,
    id: &NodeId,
    node: Option<(&PenNode, bool, usize)>,
    extra: usize,
    capture_entry: bool,
) -> Option<usize> {
    if !eligible(state) || id.as_str().len() > 16 * 1024 {
        return None;
    }
    let mut bytes = 128usize.checked_mul(1024)?.checked_add(extra)?;
    for component in state.components.components.iter().filter(|_| capture_entry) {
        bytes = bytes.checked_add(
            std::mem::size_of::<NodeId>() + component.id.as_str().len().checked_mul(3)?,
        )?;
    }
    if let Some(index) = if capture_entry {
        catalog::position(state, id).ok()?
    } else {
        None
    } {
        let old = &state.components.components[index];
        bytes = bytes
            .checked_add(
                crate::history_snapshot::command_capture_node_bytes(
                    &old.root,
                    false,
                    LIMIT.checked_sub(bytes)?,
                )
                .ok()?,
            )?
            .checked_add(old.name.len().checked_mul(8)?)?;
    }
    if let Some((node, metadata, copies)) = node {
        bytes = bytes.checked_add(
            crate::history_snapshot::command_capture_node_bytes(
                node,
                metadata,
                LIMIT.checked_sub(bytes)?,
            )
            .ok()?
            .checked_mul(copies)?,
        )?;
    }
    (bytes <= LIMIT).then_some(bytes)
}
pub(super) fn forecast(state: &EditorState, command: &C) -> Option<usize> {
    match command {
        C::CreateComponent { node_id, name } => {
            if name.trim().is_empty() {
                return None;
            }
            let node = crate::walkers::find_node(state.active_children(), node_id)?;
            if !matches!(
                node,
                PenNode::Frame(_) | PenNode::Group(_) | PenNode::Rectangle(_)
            ) {
                return None;
            }
            // A create changes only the source header, not its canonical descendants.
            bound(
                state,
                node_id,
                Some((node, true, 2)),
                name.len().checked_mul(8)?,
                true,
            )
        }
        C::DeleteComponent { component_id } => {
            state.components.find_by_id(component_id)?;
            let source =
                crate::component_backing::find_node_and_location(&state.doc, component_id.as_str())
                    .map(|(node, _)| (node, true, 1));
            bound(state, component_id, source, 0, true)
        }
        C::DetachComponent { node_id } => {
            let spec = state.component_edit_target_spec(node_id, state.ui.active_page_index)?;
            let node = patch::node(patch::roots(&state.doc, &spec.scope).ok()?, &spec.path)?;
            // Materializing a Ref owns different topology/resources. It keeps
            // the ordinary real mutator and full durable fallback.
            if matches!(node, PenNode::Ref(_))
                || (state.components.find_by_id(node_id).is_none()
                    && !matches!(node, PenNode::Frame(frame) if frame.reusable == Some(true)))
                || !matches!(
                    node,
                    PenNode::Frame(_) | PenNode::Group(_) | PenNode::Rectangle(_)
                )
            {
                return None;
            }
            bound(state, node_id, Some((node, true, 1)), 0, true)
        }
        C::InstantiateComponent { component_id } => {
            let source = state.components.resolved_root(&state.doc, component_id)?;
            let copies = if state.components.is_document_backed(component_id) {
                2
            } else {
                1
            };
            bound(state, component_id, Some((source, false, copies)), 0, true)
        }
        _ => None,
    }
}
impl Pending {
    pub(super) fn capture(state: &EditorState, command: &C) -> Option<Plan> {
        forecast(state, command)?;
        if let C::DetachComponent { node_id } = command {
            let spec = state.component_edit_target_spec(node_id, state.ui.active_page_index)?;
            return Self::capture_detach(state, node_id, &spec.path);
        }
        let (request, id, targets, prototype, catalog_delta) = match command {
            C::CreateComponent { node_id, name } => {
                let (node, location) =
                    crate::component_backing::find_node_and_location(&state.doc, node_id.as_str())?;
                let scope = scope(&location, state)?;
                if scope != active(state)
                    || !matches!(
                        node,
                        PenNode::Frame(_) | PenNode::Group(_) | PenNode::Rectangle(_)
                    )
                {
                    return None;
                }
                (
                    Request::Create {
                        id: node_id.clone(),
                        name: name.clone(),
                    },
                    node_id.clone(),
                    vec![header(scope, location.child_path, false)],
                    None,
                    true,
                )
            }
            C::DeleteComponent { component_id } => {
                let targets = crate::component_backing::find_node_and_location(
                    &state.doc,
                    component_id.as_str(),
                )
                .and_then(|(node, location)| {
                    if matches!(node, PenNode::Frame(_)) {
                        Some(header(scope(&location, state)?, location.child_path, false))
                    } else {
                        None
                    }
                })
                .into_iter()
                .collect();
                (
                    Request::Delete(component_id.clone()),
                    component_id.clone(),
                    targets,
                    None,
                    true,
                )
            }
            C::InstantiateComponent { component_id } => {
                let source = state.components.resolved_root(&state.doc, component_id)?;
                let scope = active(state);
                let index = patch::roots(&state.doc, &scope).ok()?.len();
                let forecast = forecast(state, command)?;
                let target = TargetSpec {
                    scope,
                    path: Box::new([]),
                    subtree: false,
                    splice: Some(SpliceSpec {
                        index,
                        remove: 0,
                        insert_limit: 1,
                        forecast,
                    }),
                    topology: None,
                };
                (
                    Request::Instantiate(component_id.clone()),
                    component_id.clone(),
                    vec![target],
                    Some(source),
                    false,
                )
            }
            _ => return None,
        };
        Self::finish_capture(state, request, id, targets, prototype, catalog_delta, true)
    }
    pub(super) fn capture_detach(state: &EditorState, id: &NodeId, path: &[usize]) -> Option<Plan> {
        let scope = active(state);
        let node = patch::node(patch::roots(&state.doc, &scope).ok()?, path)?;
        if node.id_str() != id.as_str() {
            return None;
        }
        let reference = matches!(node, PenNode::Ref(_));
        let registered = state.components.find_by_id(id).is_some();
        if !reference
            && !matches!(
                node,
                PenNode::Frame(_) | PenNode::Group(_) | PenNode::Rectangle(_)
            )
        {
            return None;
        }
        if !reference
            && !registered
            && !matches!(node,PenNode::Frame(frame) if frame.reusable==Some(true))
        {
            return None;
        }
        bound(state, id, Some((node, !reference, 1)), 0, !reference)?;
        Self::finish_capture(
            state,
            Request::Detach,
            id.clone(),
            vec![header(scope, path.into(), reference)],
            None,
            !reference,
            !reference,
        )
    }
    fn finish_capture(
        state: &EditorState,
        request: Request,
        id: NodeId,
        targets: Vec<TargetSpec>,
        prototype: Option<&PenNode>,
        catalog_delta: bool,
        check_entry: bool,
    ) -> Option<Plan> {
        let old = if check_entry {
            Entry::capture(state, &id, None).ok()?
        } else {
            None
        };
        let anchor = match catalog::position(state, &id).ok()? {
            Some(index) => catalog::anchors(state, &id, index),
            None => catalog::append_anchors(state, &id),
        };
        let input = prototype.map(|node| {
            // Owned prototypes can share the already captured one-entry image.
            if !state.components.is_document_backed(&id) {
                if let Some(old) = &old {
                    return Arc::clone(&old.component.root);
                }
            }
            SharedNode::capture(node, None)
        });
        let mut scopes = Vec::new();
        for target in &targets {
            if !scopes.contains(&target.scope) {
                scopes.push(target.scope.clone());
            }
            if target.splice.is_none() {
                let node =
                    patch::node(patch::roots(&state.doc, &target.scope).ok()?, &target.path)?;
                let mut wanted = identity::Wanted::new();
                wanted
                    .entry(target.scope.clone())
                    .or_default()
                    .insert(node.id_str().to_owned());
                if identity::Index::new(&state.doc, wanted)
                    .ok()?
                    .path(&target.scope, node.id_str())
                    .ok()?
                    .as_ref()
                    != target.path.as_ref()
                {
                    return None;
                }
            }
        }
        Some(Plan {
            pending: Self {
                request,
                id,
                old,
                anchor,
                order: state
                    .components
                    .components
                    .iter()
                    .filter(|_| check_entry)
                    .map(|component| component.id.clone())
                    .collect(),
                input,
                catalog_delta,
                check_entry,
            },
            scopes,
            targets,
        })
    }
    pub(super) fn request_matches(&self, command: &C) -> bool {
        match (&self.request, command) {
            (Request::Detach, C::DetachComponent { node_id }) => self.id == *node_id,
            (
                Request::Create { id, name },
                C::CreateComponent {
                    node_id,
                    name: new_name,
                },
            ) => id == node_id && name == new_name,
            (Request::Delete(id), C::DeleteComponent { component_id })
            | (Request::Instantiate(id), C::InstantiateComponent { component_id }) => {
                id == component_id
            }
            _ => false,
        }
    }
    pub(super) fn matches(&self, state: &EditorState) -> bool {
        if self.check_entry
            && self.order.iter().ne(state
                .components
                .components
                .iter()
                .map(|component| &component.id))
        {
            return false;
        }
        if !self.check_entry {
            return true;
        }
        let current = match catalog::position(state, &self.id) {
            Ok(current) => current,
            Err(_) => return false,
        };
        let same = match (&self.old, current) {
            (None, None) => true,
            (Some(old), Some(index)) => old.matches(state, index),
            _ => false,
        };
        same && self.input.as_ref().is_none_or(|input| {
            state
                .components
                .resolved_root(&state.doc, &self.id)
                .is_some_and(|node| input.equals_node(node))
        })
    }
    pub(super) fn compact(
        self,
        state: &EditorState,
    ) -> Result<Option<catalog::Patch>, ScopedHistoryError> {
        if !self.catalog_delta {
            return Ok(None);
        }
        catalog::Patch::compact(state, self.id, self.old, self.anchor)
    }
}
