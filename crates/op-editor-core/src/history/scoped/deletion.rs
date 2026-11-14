//! Exact multi-target deletion inverse; only actually deleted subtrees are
//! retained, with stable parent/neighbor identities for selective undo.
use super::{
    clones::ClonePatch,
    identity,
    patch::{self, Scope},
    splice::SplicePatch,
    ScopedHistoryError,
};
use crate::{EditorCommand as C, EditorState, PenNodeExt, SelectionState, SharedNode};
use jian_ops_schema::node::PenNode;
use std::{collections::HashMap, mem, sync::Arc};
#[path = "deletion_plan.rs"]
mod plan;
#[derive(Debug)]
struct Removed {
    parent: Box<[usize]>,
    parent_id: Option<String>,
    parent_kind: Option<mem::Discriminant<PenNode>>,
    index: usize,
    total: usize,
    left: Option<String>,
    right: Option<String>,
    old: Arc<SharedNode>,
    changed: Arc<Vec<String>>,
}
#[derive(Debug)]
pub(super) struct PendingDeletion {
    scope: Scope,
    selection: SelectionState,
    cut: bool,
    removed: Vec<Removed>,
}
pub(crate) fn forecast(state: &EditorState, command: &C) -> Option<usize> {
    plan::build(state, command).map(|plan| plan.forecast)
}
impl PendingDeletion {
    pub(super) fn capture(state: &EditorState, command: &C) -> Option<Self> {
        let plan = plan::build(state, command)?;
        let removed = plan
            .targets
            .into_iter()
            .map(|target| {
                let mut changed = Vec::new();
                identity::collect_ids(target.node, &mut changed);
                changed.sort_unstable();
                changed.dedup();
                Removed {
                    parent: target.path[..target.path.len() - 1].into(),
                    parent_id: target.parent.map(|parent| parent.id_str().to_owned()),
                    parent_kind: target.parent.map(mem::discriminant),
                    index: target.index,
                    total: target.siblings.len(),
                    left: target
                        .index
                        .checked_sub(1)
                        .map(|at| target.siblings[at].id_str().to_owned()),
                    right: target
                        .siblings
                        .get(target.index + 1)
                        .map(|node| node.id_str().to_owned()),
                    old: SharedNode::capture(target.node, None),
                    changed: Arc::new(changed),
                }
            })
            .collect();
        Some(Self {
            scope: plan.scope,
            selection: state.selection.clone(),
            cut: matches!(command, C::CutSelected),
            removed,
        })
    }
    pub(super) fn request_matches(&self, state: &EditorState, command: &C) -> bool {
        ((self.cut && matches!(command, C::CutSelected))
            || (!self.cut && matches!(command, C::DeleteSelected)))
            && state.selection == self.selection
    }
    pub(super) fn matches(&self, state: &EditorState) -> bool {
        patch::roots(&state.doc, &self.scope).is_ok_and(|roots| {
            self.removed.iter().all(|removed| {
                let nodes = if removed.parent_id.is_none() {
                    Some(roots)
                } else {
                    patch::node(roots, &removed.parent)
                        .filter(|node| {
                            Some(node.id_str()) == removed.parent_id.as_deref()
                                && Some(mem::discriminant(*node)) == removed.parent_kind
                        })
                        .and_then(|node| node.children())
                        .map(Vec::as_slice)
                };
                nodes.is_some_and(|nodes| {
                    nodes.len() == removed.total
                        && nodes
                            .get(removed.index)
                            .is_some_and(|node| removed.old.equals_node(node))
                })
            })
        })
    }
    pub(super) fn compact(
        self,
        state: &EditorState,
    ) -> Result<Option<ClonePatch>, ScopedHistoryError> {
        if self.matches(state) {
            return Ok(None);
        }
        let roots = patch::roots(&state.doc, &self.scope)?;
        let mut wanted = identity::Wanted::new();
        let entries = wanted.entry(self.scope.clone()).or_default();
        for removed in &self.removed {
            entries.extend(removed.changed.iter().cloned());
            if let Some(id) = &removed.parent_id {
                entries.insert(id.clone());
            }
        }
        let index = identity::Index::new(&state.doc, wanted)?;
        if self
            .removed
            .iter()
            .flat_map(|removed| removed.changed.iter())
            .any(|id| index.contains(&self.scope, id))
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let mut counts = HashMap::new();
        for removed in &self.removed {
            *counts.entry(removed.parent_id.clone()).or_insert(0usize) += 1;
        }
        let mut groups = Vec::with_capacity(self.removed.len());
        for removed in self.removed {
            let parent = match &removed.parent_id {
                Some(id) => index.path(&self.scope, id)?,
                None => Box::new([]),
            };
            let nodes = if removed.parent_id.is_none() {
                roots
            } else {
                patch::node(roots, &parent)
                    .filter(|node| Some(mem::discriminant(*node)) == removed.parent_kind)
                    .and_then(|node| node.children())
                    .ok_or(ScopedHistoryError::NodePathChanged)?
                    .as_slice()
            };
            let total = removed
                .total
                .checked_sub(counts[&removed.parent_id])
                .ok_or(ScopedHistoryError::NodePathChanged)?;
            if nodes.len() != total {
                return Err(ScopedHistoryError::NodePathChanged);
            }
            let id = removed.old.id_str().to_owned();
            groups.push(SplicePatch {
                scope: self.scope.clone(),
                parent,
                parent_id: removed.parent_id,
                parent_kind: removed.parent_kind,
                index: removed.index,
                expected_total: total,
                expected_present: true,
                expected_ids: Vec::new(),
                left: removed.left,
                right: removed.right,
                old_present: true,
                old: vec![removed.old],
                logical_ids: vec![id],
                changed_ids: removed.changed,
                origin_absent: false,
                recent: Vec::new(),
                sealed: true,
            });
        }
        Ok(Some(ClonePatch { groups }))
    }
    pub(super) fn restore(self, state: &mut EditorState) -> Result<(), ScopedHistoryError> {
        if let Some(patch) = self.compact(state)? {
            let mut patches = vec![patch::Patch::Clones(patch)];
            let catalog =
                super::splice_exchange::Catalog::new(&state.history, &patches, &state.doc);
            if let Some(prepared) =
                super::splice_exchange::Prepared::capture(&state.doc, &patches, &catalog, false)?
            {
                prepared.exchange(&mut state.doc, &mut patches);
            }
        }
        Ok(())
    }
}

/// Every possible selected canonical subtree is free of catalog definitions.
/// Ambiguous/virtual mutation routes are still rejected by the real planner.
pub(super) fn catalog_preserving(state: &EditorState) -> bool {
    let registered: std::collections::HashSet<_> = state
        .components
        .components
        .iter()
        .map(|component| component.id.as_str())
        .collect();
    let selected: std::collections::HashSet<_> =
        state.selection.set.iter().map(|id| id.as_str()).collect();
    let mut pending = vec![state.active_children().iter()];
    while let Some(siblings) = pending.last_mut() {
        if let Some(node) = siblings.next() {
            if selected.contains(node.id_str()) {
                let mut subtree = vec![node];
                while let Some(node) = subtree.pop() {
                    if registered.contains(node.id_str())
                        || matches!(node,PenNode::Frame(frame) if frame.reusable==Some(true))
                    {
                        return false;
                    }
                    if let Some(children) = node.children() {
                        subtree.extend(children);
                    }
                }
            } else if let Some(children) = node.children() {
                pending.push(children.iter());
            }
        } else {
            pending.pop();
        }
    }
    true
}
