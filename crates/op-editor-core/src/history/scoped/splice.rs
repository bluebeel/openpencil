//! Bounded structural edits retain the affected range, never its untouched page.
use super::{patch, ScopedHistoryError};
use crate::{EditorState, PenNodeExt, SharedNode};
use jian_ops_schema::{node::PenNode, PenDocument};
use std::{mem, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpliceSpec {
    pub index: usize,
    pub remove: usize,
    pub insert_limit: usize,
    pub forecast: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SplicePatch {
    pub scope: patch::Scope,
    pub parent: Box<[usize]>,
    pub(super) parent_id: Option<String>,
    pub(super) parent_kind: Option<mem::Discriminant<PenNode>>,
    pub(super) index: usize,
    pub(super) expected_total: usize,
    pub(super) expected_present: bool,
    pub(super) expected_ids: Vec<String>,
    pub(super) left: Option<String>,
    pub(super) right: Option<String>,
    pub(super) old_present: bool,
    pub old: Vec<Arc<SharedNode>>,
    pub(super) logical_ids: Vec<String>,
    pub(super) changed_ids: Arc<Vec<String>>,
    pub(super) origin_absent: bool,
    pub(super) recent: Vec<super::splice_order::Anchors>,
    pub(super) sealed: bool,
}
#[derive(Debug)]
pub(super) struct PendingSplice {
    patch: SplicePatch,
    insert_limit: usize,
}

pub(crate) fn payload_bound(node: &PenNode) -> Option<usize> {
    crate::history_snapshot::command_capture_node_bytes(
        node,
        false,
        crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE,
    )
    .ok()
}

fn id(node: Option<&PenNode>) -> Result<Option<String>, ScopedHistoryError> {
    node.map(|node| {
        (node.id_str().len() <= 16 * 1024)
            .then(|| node.id_str().to_owned())
            .ok_or(ScopedHistoryError::NodePathChanged)
    })
    .transpose()
}
impl SplicePatch {
    fn list<'a>(&self, doc: &'a PenDocument) -> Result<(&'a [PenNode], bool), ScopedHistoryError> {
        let roots = patch::roots(doc, &self.scope)?;
        if self.parent.is_empty() {
            return Ok((roots, true));
        }
        let parent = patch::node(roots, &self.parent).ok_or(ScopedHistoryError::NodePathChanged)?;
        if Some(parent.id_str()) != self.parent_id.as_deref()
            || Some(mem::discriminant(parent)) != self.parent_kind
            || !parent.is_container()
            || matches!(parent, PenNode::Ref(_))
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        Ok((
            parent.children().map_or(&[], Vec::as_slice),
            parent.children().is_some(),
        ))
    }
    pub(super) fn validate(&self, doc: &PenDocument) -> Result<(), ScopedHistoryError> {
        if self.sealed {
            return self.validate_identity(doc);
        }
        let (children, present) = self.list(doc)?;
        let end = self
            .index
            .checked_add(self.expected_ids.len())
            .ok_or(ScopedHistoryError::NodePathChanged)?;
        if present != self.expected_present
            || children.len() != self.expected_total
            || end > children.len()
            || children[self.index..end]
                .iter()
                .zip(&self.expected_ids)
                .any(|(node, id)| node.id_str() != id)
            || self
                .index
                .checked_sub(1)
                .and_then(|i| children.get(i))
                .map(PenNodeExt::id_str)
                != self.left.as_deref()
            || children.get(end).map(PenNodeExt::id_str) != self.right.as_deref()
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        Ok(())
    }
    pub(super) fn storage_bytes(&self) -> usize {
        self.recent.capacity() * mem::size_of::<super::splice_order::Anchors>()
            + self
                .recent
                .iter()
                .map(|anchor| {
                    anchor.ids.capacity() * mem::size_of::<String>()
                        + anchor.ids.iter().map(String::capacity).sum::<usize>()
                        + anchor.left.as_ref().map_or(0, String::capacity)
                        + anchor.right.as_ref().map_or(0, String::capacity)
                })
                .sum::<usize>()
            + self.logical_ids.capacity() * mem::size_of::<String>()
            + self.logical_ids.iter().map(String::capacity).sum::<usize>()
            + self.changed_ids.capacity() * mem::size_of::<String>()
            + self.changed_ids.iter().map(String::capacity).sum::<usize>()
            + self.parent.len() * mem::size_of::<usize>()
            + self.parent_id.as_ref().map_or(0, String::capacity)
            + self.expected_ids.capacity() * mem::size_of::<String>()
            + self
                .expected_ids
                .iter()
                .map(String::capacity)
                .sum::<usize>()
            + self.left.as_ref().map_or(0, String::capacity)
            + self.right.as_ref().map_or(0, String::capacity)
            + self.old.capacity() * mem::size_of::<Arc<SharedNode>>()
            + match &self.scope {
                patch::Scope::Root => 0,
                patch::Scope::Page { id, .. } => id.capacity(),
            }
    }
}
impl PendingSplice {
    pub(super) fn capture(
        state: &EditorState,
        scope: patch::Scope,
        parent: Box<[usize]>,
        spec: &SpliceSpec,
    ) -> Result<Self, ScopedHistoryError> {
        let roots = patch::roots(&state.doc, &scope)?;
        let (parent_id, parent_kind) = if parent.is_empty() {
            (None, None)
        } else {
            let node = patch::node(roots, &parent).ok_or(ScopedHistoryError::NodePathChanged)?;
            (id(Some(node))?, Some(mem::discriminant(node)))
        };
        if let Some(id) = &parent_id {
            let mut wanted = super::identity::Wanted::new();
            wanted.entry(scope.clone()).or_default().insert(id.clone());
            if super::identity::Index::new(&state.doc, wanted)?
                .path(&scope, id)?
                .as_ref()
                != parent.as_ref()
            {
                return Err(ScopedHistoryError::NodePathChanged);
            }
        }
        let mut patch = SplicePatch {
            scope,
            parent,
            parent_id,
            parent_kind,
            index: spec.index,
            expected_total: 0,
            expected_present: true,
            expected_ids: Vec::new(),
            left: None,
            right: None,
            old_present: true,
            old: Vec::new(),
            logical_ids: Vec::new(),
            changed_ids: Arc::new(Vec::new()),
            origin_absent: false,
            recent: Vec::new(),
            sealed: false,
        };
        let (children, present) = patch.list(&state.doc)?;
        let end = spec
            .index
            .checked_add(spec.remove)
            .ok_or(ScopedHistoryError::NodePathChanged)?;
        if end > children.len()
            || spec.insert_limit > 4096
            || spec.remove > 1
            || !children[spec.index..end]
                .iter()
                .all(|node| payload_bound(node).is_some())
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let mut affected_ids = Vec::new();
        for node in &children[spec.index..end] {
            super::identity::collect_ids(node, &mut affected_ids);
        }
        let mut affected_wanted = super::identity::Wanted::new();
        affected_wanted
            .entry(patch.scope.clone())
            .or_default()
            .extend(affected_ids.iter().cloned());
        let identities = super::identity::Index::new(&state.doc, affected_wanted)?;
        for id in &affected_ids {
            identities.path(&patch.scope, id)?;
        }
        let old = children[spec.index..end]
            .iter()
            .map(|node| SharedNode::capture(node, None))
            .collect();
        let expected_ids = children[spec.index..end]
            .iter()
            .map(|node| id(Some(node)).map(Option::unwrap))
            .collect::<Result<_, _>>()?;
        let left = id(spec.index.checked_sub(1).and_then(|i| children.get(i)))?;
        let right = id(children.get(end))?;
        patch.expected_total = children.len();
        patch.expected_present = present;
        patch.old_present = present;
        patch.expected_ids = expected_ids;
        patch.old = old;
        patch.logical_ids = patch.expected_ids.clone();
        let mut changed = affected_ids;
        changed.sort_unstable();
        changed.dedup();
        patch.changed_ids = Arc::new(changed);
        patch.origin_absent = !present;
        patch.left = left;
        patch.right = right;
        Ok(Self {
            patch,
            insert_limit: spec.insert_limit,
        })
    }
    pub(super) fn matches(&self, state: &EditorState) -> bool {
        self.patch.validate(&state.doc).is_ok()
            && self.patch.list(&state.doc).is_ok_and(|(children, _)| {
                self.patch
                    .old
                    .iter()
                    .zip(&children[self.patch.index..self.patch.index + self.patch.old.len()])
                    .all(|(old, node)| old.equals_node(node))
            })
    }
    pub(super) fn counts(&self) -> (usize, usize) {
        fn nodes(node: &SharedNode) -> usize {
            1 + node.children().map_or(0, |children| {
                children.iter().map(|child| nodes(child)).sum::<usize>()
            })
        }
        (1, self.patch.old.iter().map(|node| nodes(node)).sum())
    }
    pub(super) fn compact(
        mut self,
        state: &EditorState,
    ) -> Result<Option<SplicePatch>, ScopedHistoryError> {
        if self.matches(state) {
            return Ok(None);
        }
        let (children, present) = self.patch.list(&state.doc)?;
        let retained = self.patch.expected_total - self.patch.old.len();
        let inserted = children
            .len()
            .checked_sub(retained)
            .ok_or(ScopedHistoryError::NodePathChanged)?;
        if inserted > self.insert_limit || self.patch.index + inserted > children.len() {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let expected_ids = children[self.patch.index..self.patch.index + inserted]
            .iter()
            .map(|node| id(Some(node)).map(Option::unwrap))
            .collect::<Result<_, _>>()?;
        self.patch.expected_ids = expected_ids;
        if self.patch.logical_ids.is_empty() {
            self.patch.logical_ids = self.patch.expected_ids.clone();
        }
        let mut changed = self.patch.changed_ids.as_ref().clone();
        for node in &children[self.patch.index..self.patch.index + inserted] {
            super::identity::collect_ids(node, &mut changed);
        }
        changed.sort_unstable();
        changed.dedup();
        self.patch.changed_ids = Arc::new(changed);
        self.patch.expected_total = children.len();
        self.patch.expected_present = present;
        self.patch.sealed = true;
        self.patch.validate(&state.doc)?;
        Ok(Some(self.patch))
    }
    pub(super) fn restore(self, state: &mut EditorState) -> Result<(), ScopedHistoryError> {
        if let Some(patch) = self.compact(state)? {
            let mut patches = vec![super::patch::Patch::Splice(patch)];
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
