//! Prepare the exact chosen source before mutation; remint its owned result.
use super::*;
use std::collections::HashSet;
pub(super) struct Plan {
    path: Box<[usize]>,
    original_id: NodeId,
    instance: Option<PenNode>,
}
pub(super) fn prepare(state: &EditorState, id: &NodeId) -> Option<Plan> {
    let path = routes::find(state.active_children(), id.as_str())?;
    let node = routes::at(state.active_children(), &path)?;
    let instance = match node {
        PenNode::Ref(reference) => {
            let lookup = crate::ref_resolve::NodeLookup::from_components_with_document_fallback(
                &state.components,
                &state.doc,
            );
            Some(crate::ref_resolve::materialize_instance(
                node,
                lookup.find(&reference.target)?,
            )?)
        }
        node if is_component_root(node)
            && (matches!(node,PenNode::Frame(frame) if frame.reusable==Some(true))
                || state.components.find_by_id(id).is_some()) =>
        {
            None
        }
        _ => return None,
    };
    Some(Plan {
        path,
        original_id: id.clone(),
        instance,
    })
}
fn remint(
    node: &mut PenNode,
    allocator: &mut dyn IdAllocator,
    taken: &mut HashSet<NodeId>,
) -> Result<(), IdAllocError> {
    node.base_mut().id = allocator.allocate(taken)?.into();
    if node.children().is_some() {
        for child in node.children_mut().expect("existing detached child slot") {
            remint(child, allocator, taken)?;
        }
    }
    Ok(())
}
pub(super) fn apply(
    state: &mut EditorState,
    mut plan: Plan,
    allocator: &mut dyn IdAllocator,
) -> Result<Option<NodeId>, IdAllocError> {
    if let Some(mut detached) = plan.instance.take() {
        let mut taken = allocator.document_collision_ids(&state.doc);
        // Allocation can fail after some owned ids change, but no live node/history
        // has changed and no fallback id is synthesized. Runtime attempts may burn.
        remint(&mut detached, allocator, &mut taken)?;
        let id = NodeId::new(detached.id_str());
        let pending = crate::history::PendingScopedSnapshot::capture_component_detach(
            state,
            &plan.original_id,
            &plan.path,
        );
        let snapshot = pending
            .is_none()
            .then(|| state.snapshot_for_undo())
            .flatten();
        *routes::at_mut(state.active_children_mut(), &plan.path)
            .expect("checked detach source route") = detached;
        state.set_single_selection(id.clone());
        state.finish_component_history(pending, snapshot);
        crate::component_mutation_receipt::record(state, &plan.original_id, &plan.path, &id, true);
        Ok(Some(id))
    } else {
        let pending = crate::history::PendingScopedSnapshot::capture_component_detach(
            state,
            &plan.original_id,
            &plan.path,
        );
        let snapshot = pending
            .is_none()
            .then(|| state.snapshot_for_undo())
            .flatten();
        set_reusable(
            routes::at_mut(state.active_children_mut(), &plan.path)
                .expect("checked component source route"),
            false,
        );
        state.components.remove(&plan.original_id);
        state.finish_component_history(pending, snapshot);
        crate::component_mutation_receipt::record(
            state,
            &plan.original_id,
            &plan.path,
            &plan.original_id,
            false,
        );
        Ok(Some(plan.original_id))
    }
}
pub(super) fn standalone(state: &mut EditorState, id: &NodeId) -> Option<NodeId> {
    let plan = prepare(state, id)?;
    if plan.instance.is_none() {
        // Unregistering a definition emits no ids, including at counter exhaustion.
        return apply(state, plan, &mut crate::DocumentIdAllocator::sequential(1))
            .ok()
            .flatten();
    }
    let mut allocator =
        crate::command_apply_legacy::LeafHighWaterAllocator(state.standalone_id_allocator().ok()?);
    state
        .observe_runtime_allocations(
            &mut allocator,
            move |state, allocator| apply(state, plan, allocator),
            |result| result.is_some(),
        )
        .ok()
        .flatten()
}
