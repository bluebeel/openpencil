//! Exact Boolean leaf-removal/root-insertion history, using the established
//! stable splice exchange instead of retaining the active page.
use super::{
    clones::ClonePatch,
    identity,
    patch::{self, Scope},
    splice::SplicePatch,
    ScopedHistoryError,
};
use crate::{EditorCommand as C, EditorState, NodeId, PenNodeExt, SharedNode};
use jian_ops_schema::node::PenNode;
use std::{collections::HashMap, mem, sync::Arc};
#[path = "boolean_plan.rs"]
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
}
#[derive(Debug)]
pub(super) struct PendingBoolean {
    scope: Scope,
    ids: Vec<NodeId>,
    generation: u64,
    revision: u64,
    page: usize,
    removed: Vec<Removed>,
    root_len: usize,
    root_removed: usize,
    root_left: Option<String>,
}
pub(crate) fn forecast(state: &EditorState, command: &C) -> Option<usize> {
    plan::build(state, command).map(|plan| plan.forecast)
}
impl PendingBoolean {
    pub(super) fn capture(state: &EditorState, command: &C) -> Option<Self> {
        let plan = plan::build(state, command)?;
        let removed = plan
            .leaves
            .into_iter()
            .map(|leaf| Removed {
                parent: leaf.parent,
                parent_id: leaf.parent_id.map(str::to_owned),
                parent_kind: leaf.parent_kind,
                index: leaf.index,
                total: leaf.total,
                left: leaf.left.map(str::to_owned),
                right: leaf.right.map(str::to_owned),
                old: SharedNode::capture_header(leaf.node),
            })
            .collect();
        Some(Self {
            scope: plan.scope,
            ids: state.selection.set.clone(),
            generation: state.document_generation(),
            revision: state.document_revision(),
            page: state.ui.active_page_index,
            removed,
            root_len: plan.root_len,
            root_removed: plan.root_removed,
            root_left: plan.root_left.map(str::to_owned),
        })
    }
    pub(super) fn request_matches(&self, state: &EditorState, command: &C) -> bool {
        plan::checked_request(
            state,
            command,
            &self.ids,
            self.generation,
            self.revision,
            self.page,
        )
    }
    fn list<'a>(&self, state: &'a EditorState, removed: &Removed) -> Option<&'a [PenNode]> {
        let roots = patch::roots(&state.doc, &self.scope).ok()?;
        if removed.parent_id.is_none() {
            return Some(roots);
        }
        let parent = patch::node(roots, &removed.parent)?;
        if Some(parent.id_str()) != removed.parent_id.as_deref()
            || Some(mem::discriminant(parent)) != removed.parent_kind
        {
            return None;
        }
        parent.children().map(Vec::as_slice)
    }
    pub(super) fn matches(&self, state: &EditorState) -> bool {
        patch::roots(&state.doc, &self.scope).is_ok_and(|roots| roots.len() == self.root_len)
            && self.removed.iter().all(|removed| {
                self.list(state, removed).is_some_and(|nodes| {
                    nodes.len() == removed.total
                        && nodes
                            .get(removed.index)
                            .is_some_and(|node| removed.old.equals_node(node))
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
        let expected = self
            .root_len
            .checked_sub(self.root_removed)
            .and_then(|len| len.checked_add(1))
            .ok_or(ScopedHistoryError::NodePathChanged)?;
        let result = roots
            .last()
            .filter(|_| roots.len() == expected)
            .filter(|node| matches!(node, PenNode::Path(_)))
            .ok_or(ScopedHistoryError::NodePathChanged)?;
        if self.ids.iter().any(|id| id.as_str() == result.id_str()) {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let result_id = result.id_str().to_owned();
        let mut wanted = identity::Wanted::new();
        let scope_wanted = wanted.entry(self.scope.clone()).or_default();
        scope_wanted.extend(self.ids.iter().map(|id| id.as_str().to_owned()));
        scope_wanted.insert(result_id.clone());
        for removed in &self.removed {
            if let Some(id) = &removed.parent_id {
                scope_wanted.insert(id.clone());
            }
        }
        let index = identity::Index::new(&state.doc, wanted)?;
        if self
            .ids
            .iter()
            .any(|id| index.contains(&self.scope, id.as_str()))
            || index.path(&self.scope, &result_id)?.as_ref() != &[roots.len() - 1]
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let mut counts = HashMap::new();
        for removed in &self.removed {
            *counts.entry(removed.parent_id.clone()).or_insert(0usize) += 1;
        }
        let mut groups = Vec::with_capacity(self.removed.len() + 1);
        for removed in self.removed {
            let parent = match &removed.parent_id {
                Some(id) => index.path(&self.scope, id)?,
                None => Box::new([]),
            };
            let nodes = if removed.parent_id.is_none() {
                roots
            } else {
                let node = patch::node(roots, &parent)
                    .filter(|node| Some(mem::discriminant(*node)) == removed.parent_kind)
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
                node.children()
                    .ok_or(ScopedHistoryError::NodePathChanged)?
                    .as_slice()
            };
            let total = removed
                .total
                .checked_sub(counts[&removed.parent_id])
                .and_then(|len| len.checked_add(usize::from(removed.parent_id.is_none())))
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
                logical_ids: vec![id.clone()],
                changed_ids: Arc::new(vec![id]),
                origin_absent: false,
                recent: Vec::new(),
                sealed: true,
            });
        }
        groups.push(SplicePatch {
            scope: self.scope,
            parent: Box::new([]),
            parent_id: None,
            parent_kind: None,
            index: roots.len() - 1,
            expected_total: roots.len(),
            expected_present: true,
            expected_ids: vec![result_id.clone()],
            left: self.root_left,
            right: None,
            old_present: true,
            old: Vec::new(),
            logical_ids: vec![result_id.clone()],
            changed_ids: Arc::new(vec![result_id]),
            origin_absent: false,
            recent: Vec::new(),
            sealed: true,
        });
        Ok(Some(ClonePatch { groups }))
    }
    pub(super) fn restore(self, state: &mut EditorState) -> Result<(), ScopedHistoryError> {
        if let Some(patch) = self.compact(state)? {
            let mut patches = vec![patch::Patch::Clones(patch)];
            let catalog = super::splice_exchange::Catalog::new(&state.history, &patches);
            if let Some(prepared) =
                super::splice_exchange::Prepared::capture(&state.doc, &patches, &catalog, false)?
            {
                prepared.exchange(&mut state.doc, &mut patches);
            }
        }
        Ok(())
    }
}
