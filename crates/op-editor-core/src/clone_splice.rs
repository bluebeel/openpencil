//! Atomic clone preparation and one affected-list insertion pass.
use crate::{walkers, EditorState, IdAllocError, IdAllocator, NodeId, PenNodeExt};
use jian_ops_schema::node::PenNode;
use std::collections::HashSet;
#[path = "clone_splice_lookup.rs"]
mod lookup;
#[path = "clone_splice_clone.rs"]
mod payload;
#[path = "clone_splice_plan.rs"]
mod plan;
use plan::Plan;
#[path = "clone_splice_trace.rs"]
mod trace;
pub(crate) use trace::Trace;
#[path = "clone_splice_forecast.rs"]
mod forecast;
pub(crate) fn forecast(state: &EditorState, command: &crate::EditorCommand) -> Option<usize> {
    forecast::bound(state, command)
}
#[derive(Debug)]
pub(crate) struct Prepared {
    plan: Plan,
    ids: Vec<NodeId>,
}
impl Prepared {
    pub(crate) fn commit(self, roots: &mut Vec<PenNode>) -> Vec<NodeId> {
        if !self.ids.is_empty() {
            self.plan.commit(roots);
        }
        self.ids
    }
}
/// None denotes actual ambiguous canonical identities and keeps the legacy
/// staged resolver. Empty ids inside Prepared are an ordinary no-op.
pub(crate) fn duplicate(
    state: &EditorState,
    allocator: &mut dyn IdAllocator,
    offset: f64,
) -> Result<Option<Prepared>, IdAllocError> {
    let roots = state.active_children();
    let wanted: HashSet<_> = state
        .selection
        .set
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect();
    let index = lookup::Index::new(roots, wanted);
    if state
        .selection
        .set
        .iter()
        .any(|id| index.ambiguous(id.as_str()))
    {
        return Ok(None);
    }
    let mut plan = Plan::new(
        index
            .wanted
            .iter()
            .filter(|id| !index.original.contains_key(id.as_str()))
            .cloned()
            .collect(),
    );
    if index.original.is_empty() {
        return Ok(Some(Prepared {
            plan,
            ids: Vec::new(),
        }));
    }
    let mut taken = allocator.document_collision_ids(&state.doc);
    let mut ids = Vec::with_capacity(state.selection.set.len());
    for target in &state.selection.set {
        if let Some(source) = plan.source(&index, target.as_str()) {
            if let Some(id) =
                plan.duplicate_source(roots, &source, allocator, &mut taken, offset)?
            {
                ids.push(id);
            }
        }
    }
    Ok(Some(Prepared { plan, ids }))
}
fn reusable(node: &PenNode) -> bool {
    matches!(node,PenNode::Frame(frame) if frame.reusable==Some(true))
}
fn paste_target(
    roots: &[PenNode],
    index: &mut lookup::Index,
    anchor: Option<&NodeId>,
) -> Option<(Box<[usize]>, Option<usize>)> {
    let Some(anchor) = anchor else {
        return Some((Box::new([]), None));
    };
    if index.ambiguous(anchor.as_str()) {
        return None;
    }
    let Some(source) = index.original.get(anchor.as_str()) else {
        return Some((Box::new([]), None));
    };
    let path = source.preorder.clone();
    let node = lookup::at(roots, &path).unwrap();
    if node.children().is_some() {
        return Some((path, None));
    }
    let insert = path.last().copied().unwrap() + 1;
    // The legacy parent query skips empty ids while unwinding. Preserve that
    // exact ancestor choice, then resolve its preorder insertion authority.
    for length in (1..path.len()).rev() {
        let parent = lookup::at(roots, &path[..length]).unwrap();
        if let Some(id) = NodeId::new_opt(parent.id_str()) {
            index.resolve_extra(roots, &id);
            if index.ambiguous(id.as_str()) {
                return None;
            }
            let parent = index.original.get(id.as_str()).unwrap().preorder.clone();
            debug_assert!(lookup::at(roots, &parent).unwrap().children().is_some());
            return Some((parent, Some(insert)));
        }
    }
    Some((Box::new([]), Some(insert)))
}
pub(crate) fn paste(
    state: &EditorState,
    allocator: &mut dyn IdAllocator,
    offset: f64,
) -> Result<Option<Prepared>, IdAllocError> {
    let roots = state.active_children();
    let mut wanted = HashSet::new();
    if let Some(anchor) = state.selection.set.first() {
        wanted.insert(anchor.as_str().to_owned());
    }
    for node in &state.clipboard {
        if reusable(node) {
            wanted.insert(node.id_str().to_owned());
        }
    }
    let mut index = lookup::Index::new(roots, wanted);
    for node in &state.clipboard {
        if reusable(node)
            && index.ambiguous(node.id_str())
            && index
                .original
                .get(node.id_str())
                .is_some_and(|source| reusable(lookup::at(roots, &source.preorder).unwrap()))
        {
            return Ok(None);
        }
    }
    let Some((parent, mut cursor)) = paste_target(roots, &mut index, state.selection.set.first())
    else {
        return Ok(None);
    };
    let mut plan = Plan::new(
        index
            .wanted
            .iter()
            .filter(|id| !index.original.contains_key(id.as_str()))
            .cloned()
            .collect(),
    );
    let mut destination = None;
    let mut taken = allocator.document_collision_ids(&state.doc);
    let mut ids = Vec::with_capacity(state.clipboard.len());
    for original in &state.clipboard {
        if reusable(original) {
            let source = plan.source(&index, original.id_str());
            let live = index.original.get(original.id_str()).map_or_else(
                || {
                    source
                        .as_ref()
                        .is_some_and(|source| reusable(plan.node(roots, source)))
                },
                |source| reusable(lookup::at(roots, &source.preorder).unwrap()),
            );
            if live {
                if let Some(id) =
                    plan.duplicate_source(roots, &source.unwrap(), allocator, &mut taken, offset)?
                {
                    ids.push(id);
                    continue;
                }
            }
        }
        let mut clone = walkers::deep_clone_with_allocator(original, allocator, &mut taken)?;
        walkers::translate_subtree(&mut clone, offset, offset);
        if let Some(id) = NodeId::new_opt(clone.id_str()) {
            ids.push(id);
        }
        let list = match destination {
            Some(list) => list,
            None => {
                let list = plan.list(roots, &parent);
                destination = Some(list);
                list
            }
        };
        plan.insert_numeric(list, cursor, clone);
        if let Some(cursor) = cursor.as_mut() {
            *cursor += 1;
        }
    }
    Ok(Some(Prepared { plan, ids }))
}

/// A prepared standalone host command exclusively owns the exact current
/// document high-water. Every output node id comes from this one allocator.
#[derive(Debug)]
struct CaptureAllocator(crate::DocumentIdAllocator);
impl crate::IdAllocator for CaptureAllocator {
    fn document_collision_ids(
        &self,
        _: &jian_ops_schema::PenDocument,
    ) -> std::collections::HashSet<NodeId> {
        std::collections::HashSet::new()
    }
    fn allocate(
        &mut self,
        taken: &mut std::collections::HashSet<NodeId>,
    ) -> Result<NodeId, IdAllocError> {
        self.0.allocate(taken)
    }
}
pub(crate) fn prepare_captured(
    state: &EditorState,
    command: &crate::EditorCommand,
) -> Option<Prepared> {
    forecast(state, command)?;
    let mut allocator = CaptureAllocator(state.standalone_id_allocator().ok()?);
    match command {
        crate::EditorCommand::DuplicateSelected { offset_px } => {
            duplicate(state, &mut allocator, *offset_px as f64)
                .ok()
                .flatten()
        }
        crate::EditorCommand::PasteClipboard { offset_px } => {
            paste(state, &mut allocator, *offset_px as f64)
                .ok()
                .flatten()
        }
        _ => None,
    }
}
