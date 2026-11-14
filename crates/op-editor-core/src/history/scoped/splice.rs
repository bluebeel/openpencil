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
    parent_id: Option<String>,
    parent_kind: Option<mem::Discriminant<PenNode>>,
    index: usize,
    expected_total: usize,
    expected_present: bool,
    expected_ids: Vec<String>,
    left: Option<String>,
    right: Option<String>,
    old_present: bool,
    pub old: Vec<Arc<SharedNode>>,
}
#[derive(Debug)]
pub(super) struct PendingSplice {
    patch: SplicePatch,
    insert_limit: usize,
}

pub(crate) fn payload_bound(node: &PenNode) -> Option<usize> {
    fn count(node: &PenNode, depth: usize, left: &mut usize) -> bool {
        if depth > 128 || *left == 0 {
            return false;
        }
        *left -= 1;
        node.children()
            .is_none_or(|children| children.iter().all(|child| count(child, depth + 1, left)))
    }
    let mut remaining = 4096;
    if !count(node, 0, &mut remaining) {
        return None;
    }
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            if self.0 > 4 * 1024 * 1024 {
                return Err(std::io::ErrorKind::OutOfMemory.into());
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut bytes = Counter(0);
    serde_json::to_writer(&mut bytes, node).ok()?;
    Some(
        bytes.0.saturating_mul(32)
            + (4096 - remaining) * mem::size_of::<SharedNode>() * 4
            + 128 * 1024,
    )
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
    /// Only called after validation under the exclusive document authority.
    pub(super) fn exchange(&mut self, doc: &mut PenDocument) {
        let roots = patch::roots_mut(doc, &self.scope).unwrap();
        let mut slot = None;
        let children = if self.parent.is_empty() {
            roots
        } else {
            let parent = patch::node_mut(roots, &self.parent).unwrap();
            let children = patch::children_slot(parent).unwrap();
            slot = Some(children);
            slot.as_mut().unwrap().get_or_insert_with(Vec::new)
        };
        let end = self.index + self.expected_ids.len();
        let reverse = children[self.index..end]
            .iter()
            .map(|node| SharedNode::capture(node, None))
            .collect();
        let restored = self
            .old
            .iter()
            .map(|node| node.materialize())
            .collect::<Vec<_>>();
        let restored_ids = restored
            .iter()
            .map(|node| node.id_str().to_owned())
            .collect();
        children.splice(self.index..end, restored);
        self.expected_total = children.len();
        self.expected_ids = restored_ids;
        self.old = reverse;
        if !self.old_present {
            debug_assert!(children.is_empty());
            *slot.unwrap() = None;
        }
        mem::swap(&mut self.old_present, &mut self.expected_present);
    }
    pub(super) fn storage_bytes(&self) -> usize {
        self.parent.len() * mem::size_of::<usize>()
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
        self.patch.expected_total = children.len();
        self.patch.expected_present = present;
        self.patch.validate(&state.doc)?;
        Ok(Some(self.patch))
    }
    pub(super) fn restore(self, state: &mut EditorState) -> Result<(), ScopedHistoryError> {
        if let Some(mut patch) = self.compact(state)? {
            patch.exchange(&mut state.doc);
        }
        Ok(())
    }
}
