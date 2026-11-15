//! Group/ungroup history retains wrapper metadata and positions, not child trees.
use super::{
    patch,
    topology_parent::{self as parent, Parent},
    ScopedHistoryError,
};
use crate::{
    history_snapshot::{command_capture_node_bytes, COMMAND_CAPTURE_ALLOWANCE},
    pen_node_ext::clone_without_children,
    PenNodeExt, SharedNode,
};
use jian_ops_schema::{node::PenNode, PenDocument};
use std::{mem, sync::Arc};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct WrapPatch {
    pub scope: patch::Scope,
    parent: Parent,
    index: usize,
    positions: Box<[usize]>,
    members: Vec<String>,
    header: Arc<SharedNode>,
    wrapped: bool,
    total: usize,
    present: bool,
    left: Option<String>,
    right: Option<String>,
}
#[derive(Debug)]
enum BeforeWrap {
    Group {
        scope: patch::Scope,
        parent: Parent,
        index: usize,
        positions: Box<[usize]>,
        members: Vec<String>,
        total: usize,
        present: bool,
        left: Option<String>,
        right: Option<String>,
    },
    Ungroup(WrapPatch),
}
#[derive(Debug)]
pub(super) struct PendingWrap {
    before: BeforeWrap,
    after: Option<WrapPatch>,
}
fn member_ids(nodes: &[PenNode]) -> Result<Vec<String>, ScopedHistoryError> {
    let mut bytes = 0usize;
    nodes
        .iter()
        .map(|node| {
            bytes = bytes.saturating_add(node.id_str().len());
            if bytes > COMMAND_CAPTURE_ALLOWANCE {
                return Err(ScopedHistoryError::NodePathChanged);
            }
            parent::identity(Some(node)).map(Option::unwrap)
        })
        .collect()
}
fn header(
    node: &PenNode,
    anchor: Option<&Arc<SharedNode>>,
    limit: usize,
) -> Result<Arc<SharedNode>, ScopedHistoryError> {
    command_capture_node_bytes(node, true, limit)
        .map_err(|_| ScopedHistoryError::NodePathChanged)?;
    Ok(SharedNode::capture(&clone_without_children(node), anchor))
}
impl WrapPatch {
    fn boundaries(
        &self,
        nodes: &[PenNode],
    ) -> Result<(Option<String>, Option<String>), ScopedHistoryError> {
        let end = if self.wrapped {
            self.index + 1
        } else {
            self.positions.last().map_or(self.index, |last| last + 1)
        };
        Ok((
            parent::identity(self.index.checked_sub(1).and_then(|index| nodes.get(index)))?,
            parent::identity(nodes.get(end))?,
        ))
    }
    pub(super) fn validate(&self, doc: &PenDocument) -> Result<(), ScopedHistoryError> {
        let (nodes, present) = self.parent.list(doc, &self.scope)?;
        if nodes.len() != self.total
            || present != self.present
            || self.boundaries(nodes)? != (self.left.clone(), self.right.clone())
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        if self.wrapped {
            let group = nodes
                .get(self.index)
                .ok_or(ScopedHistoryError::NodePathChanged)?;
            if !group.is_group()
                || group.id_str() != self.header.id_str()
                || group.children().is_some() != self.header.children().is_some()
                || group.children().map_or(0, Vec::len) != self.members.len()
                || group
                    .children()
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .zip(&self.members)
                    .any(|(node, id)| node.id_str() != id)
                || command_capture_node_bytes(group, true, COMMAND_CAPTURE_ALLOWANCE).is_err()
            {
                return Err(ScopedHistoryError::NodePathChanged);
            }
        } else if self.positions.len() != self.members.len()
            || self
                .positions
                .iter()
                .zip(&self.members)
                .any(|(index, id)| nodes.get(*index).is_none_or(|node| node.id_str() != id))
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        Ok(())
    }
    pub(super) fn exchange(&mut self, doc: &mut PenDocument) {
        let nodes = parent::list_mut(doc, &self.scope, &self.parent.path);
        if self.wrapped {
            let mut group = nodes.remove(self.index);
            self.header = header(&group, Some(&self.header), COMMAND_CAPTURE_ALLOWANCE).unwrap();
            let members = patch::children_slot(&mut group)
                .and_then(Option::take)
                .unwrap_or_default();
            if self
                .positions
                .iter()
                .enumerate()
                .all(|(offset, index)| *index == self.index + offset)
            {
                // The ordinary ungroup path moves one range in one Vec splice.
                drop(nodes.splice(self.index..self.index, members));
            } else {
                for (index, node) in self.positions.iter().zip(members) {
                    nodes.insert(*index, node);
                }
            }
        } else {
            // One in-place compaction, including noncontiguous selections;
            // repeated Vec::remove would shift the same siblings per member.
            let mut members = Vec::with_capacity(self.positions.len());
            let mut index = 0usize;
            let mut position = 0usize;
            members.extend(nodes.extract_if(.., |_| {
                let take = self.positions.get(position) == Some(&index);
                index += 1;
                if take {
                    position += 1;
                }
                take
            }));
            let mut group = self.header.materialize();
            if self.header.children().is_some() {
                *patch::children_slot(&mut group).unwrap() = Some(members);
            } else {
                debug_assert!(members.is_empty());
            }
            nodes.insert(self.index, group);
        }
        self.wrapped = !self.wrapped;
        self.total = nodes.len();
        self.present = true;
        let (left, right) = self.boundaries(nodes).unwrap();
        self.left = left;
        self.right = right;
    }
    pub(super) fn roots(&self) -> &[Arc<SharedNode>] {
        std::slice::from_ref(&self.header)
    }
    pub(super) fn cohort_bound(
        &self,
        doc: &PenDocument,
        index: &super::identity::Index,
        bound: &mut super::cohort::Bound,
    ) -> Option<()> {
        bound.parent(doc, &self.scope, self.parent.id.as_deref(), index)?;
        bound.add(
            self.positions
                .len()
                .checked_mul(mem::size_of::<PenNode>() * 4)?,
        )
    }
    pub(super) fn bytes(&self) -> usize {
        parent::scope_bytes(&self.scope)
            + self.parent.bytes()
            + self.positions.len() * mem::size_of::<usize>()
            + self.members.capacity() * mem::size_of::<String>()
            + self.members.iter().map(String::capacity).sum::<usize>()
            + self.left.as_ref().map_or(0, String::capacity)
            + self.right.as_ref().map_or(0, String::capacity)
    }
    pub(super) fn want_identities(&self, wanted: &mut super::identity::Wanted) {
        self.parent.want(&self.scope, wanted);
    }
    pub(super) fn rebase_identity(
        &mut self,
        index: &super::identity::Index,
    ) -> Result<(), ScopedHistoryError> {
        self.parent.rebase_identity(&self.scope, index)
    }
    pub(super) fn conflicts_splice(&self, splice: &super::splice::SplicePatch) -> bool {
        self.scope.same_authority(&splice.scope)
            && (self.parent.id == splice.parent_id
                || splice.contains_changed(self.header.id_str())
                || self
                    .parent
                    .id
                    .as_deref()
                    .is_some_and(|id| splice.contains_changed(id))
                || self.members.iter().any(|id| splice.contains_changed(id)))
    }
    pub(super) fn conflicts_node(&self, node: &super::patch::NodePatch) -> bool {
        if !node.scope.same_authority(&self.scope) {
            return false;
        }
        if node.expected_id == self.header.id_str() || node.old.id_str() == self.header.id_str() {
            return true;
        }
        if node.metadata_only {
            return false;
        }
        if node.expected_id != node.old.id_str()
            && self
                .members
                .iter()
                .any(|id| id == &node.expected_id || id == node.old.id_str())
        {
            return true;
        }
        self.parent.id.iter().any(|id| node.contains_affected(id))
    }
    pub(super) fn actor_regions(&self) -> [Option<super::actor_claim::Region<'_>>; 2] {
        use super::actor_claim::{Kind, Region};
        [
            Some(Region {
                scope: &self.scope,
                path: &self.parent.path,
                kind: Kind::Children,
            }),
            None,
        ]
    }
    pub(super) fn rebase(&mut self, reorders: &patch::Reorders) {
        reorders.rebase(&self.scope, &mut self.parent.path);
    }
}
impl PendingWrap {
    pub(super) fn capture_group(
        doc: &PenDocument,
        scope: patch::Scope,
        parent: Box<[usize]>,
        positions: Box<[usize]>,
    ) -> Result<Self, ScopedHistoryError> {
        if positions.is_empty() || positions.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let parent = Parent::capture(doc, &scope, parent)?;
        let (nodes, present) = parent.list(doc, &scope)?;
        let mut bytes = 0usize;
        let members = positions
            .iter()
            .map(|index| {
                let node = nodes
                    .get(*index)
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
                bytes = bytes.saturating_add(node.id_str().len());
                if bytes > COMMAND_CAPTURE_ALLOWANCE {
                    return Err(ScopedHistoryError::NodePathChanged);
                }
                parent::identity(Some(node)).map(Option::unwrap)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let index = positions[0];
        let left = parent::identity(index.checked_sub(1).and_then(|index| nodes.get(index)))?;
        let right = parent::identity(
            nodes
                .iter()
                .enumerate()
                .skip(index + 1)
                .find(|(index, _)| positions.binary_search(index).is_err())
                .map(|(_, node)| node),
        )?;
        Ok(Self {
            before: BeforeWrap::Group {
                scope,
                parent,
                index,
                positions,
                members,
                total: nodes.len(),
                present,
                left,
                right,
            },
            after: None,
        })
    }
    pub(super) fn capture_ungroup(
        doc: &PenDocument,
        scope: patch::Scope,
        parent: Box<[usize]>,
        index: usize,
    ) -> Result<Self, ScopedHistoryError> {
        let parent = Parent::capture(doc, &scope, parent)?;
        let (nodes, present) = parent.list(doc, &scope)?;
        let group = nodes
            .get(index)
            .filter(|node| node.is_group())
            .ok_or(ScopedHistoryError::NodePathChanged)?;
        let header = header(group, None, COMMAND_CAPTURE_ALLOWANCE)?;
        let members = member_ids(group.children().map_or(&[], Vec::as_slice))?;
        let positions = (index..index + members.len())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let patch = WrapPatch {
            scope,
            parent,
            index,
            positions,
            members,
            header,
            wrapped: true,
            total: nodes.len(),
            present,
            left: parent::identity(index.checked_sub(1).and_then(|index| nodes.get(index)))?,
            right: parent::identity(nodes.get(index + 1))?,
        };
        Ok(Self {
            before: BeforeWrap::Ungroup(patch),
            after: None,
        })
    }
    pub(super) fn matches(&self, doc: &PenDocument) -> bool {
        match &self.before {
            BeforeWrap::Group {
                scope,
                parent,
                positions,
                members,
                total,
                present,
                ..
            } => parent.list(doc, scope).is_ok_and(|(nodes, current)| {
                nodes.len() == *total
                    && current == *present
                    && positions.iter().zip(members).all(|(index, id)| {
                        nodes.get(*index).is_some_and(|node| node.id_str() == id)
                    })
            }),
            BeforeWrap::Ungroup(patch) => {
                patch.validate(doc).is_ok()
                    && patch
                        .parent
                        .list(doc, &patch.scope)
                        .is_ok_and(|(nodes, _)| {
                            patch
                                .header
                                .equals_node(&clone_without_children(&nodes[patch.index]))
                        })
            }
        }
    }
    pub(super) fn apply_direct(&mut self, state: &mut crate::EditorState) -> bool {
        use crate::IdAllocator;
        if self.after.is_some() || !self.matches(&state.doc) {
            return false;
        }
        let mut patch = match &self.before {
            BeforeWrap::Group {
                scope,
                parent,
                index,
                positions,
                members,
                total,
                present,
                ..
            } => {
                let Ok(mut allocator) = state.standalone_id_allocator() else {
                    return false;
                };
                // This exclusive command emits exactly one numeric id above
                // every current canonical node/page id; no collision set is needed.
                let Ok(id) = allocator.allocate(&mut std::collections::HashSet::new()) else {
                    return false;
                };
                let group =
                    crate::pen_node_ext::make_group(id.as_str().to_owned(), "Group", Vec::new());
                let (nodes, _) = parent.list(&state.doc, scope).unwrap();
                let mut patch = WrapPatch {
                    scope: scope.clone(),
                    parent: parent.clone(),
                    index: *index,
                    positions: positions.clone(),
                    members: members.clone(),
                    header: header(&group, None, 64 * 1024).unwrap(),
                    wrapped: false,
                    total: *total,
                    present: *present,
                    left: None,
                    right: None,
                };
                let (left, right) = patch.boundaries(nodes).unwrap();
                patch.left = left;
                patch.right = right;
                patch
            }
            BeforeWrap::Ungroup(patch) => patch.clone(),
        };
        patch.exchange(&mut state.doc);
        if patch.wrapped {
            if let Some(highest) = crate::walkers::parse_n_id(patch.header.id_str()) {
                state.record_runtime_id_high_water(highest);
            }
            let id = crate::NodeId::new(patch.header.id_str());
            state.selection = crate::SelectionState {
                anchor: id.clone(),
                set: vec![id],
            };
        } else {
            let ids = patch
                .members
                .iter()
                .filter_map(|id| crate::NodeId::new_opt(id.as_str()))
                .collect::<Vec<_>>();
            if ids.is_empty() {
                state.clear_selection();
            } else {
                state.selection = crate::SelectionState {
                    anchor: ids.last().cloned().unwrap(),
                    set: ids,
                };
            }
        }
        self.after = Some(patch);
        true
    }
    pub(super) fn compact(
        mut self,
        doc: &PenDocument,
    ) -> Result<Option<WrapPatch>, ScopedHistoryError> {
        if let Some(patch) = self.after.take() {
            patch.validate(doc)?;
            return Ok(Some(patch));
        }
        if self.matches(doc) {
            return Ok(None);
        }
        let result = match self.before {
            BeforeWrap::Group {
                scope,
                parent,
                index,
                positions,
                members,
                total,
                left,
                right,
                ..
            } => {
                let (nodes, present) = parent.list(doc, &scope)?;
                let group = nodes
                    .get(index)
                    .filter(|node| node.is_group())
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
                let header = header(group, None, 64 * 1024)?;
                let count = members.len();
                WrapPatch {
                    scope,
                    parent,
                    index,
                    positions,
                    members,
                    header,
                    wrapped: true,
                    total: total - count + 1,
                    present,
                    left,
                    right,
                }
            }
            BeforeWrap::Ungroup(mut patch) => {
                patch.wrapped = false;
                patch.total = patch.total - 1 + patch.members.len();
                patch.present = true;
                patch
            }
        };
        result.validate(doc)?;
        Ok(Some(result))
    }
}

impl WrapPatch {
    pub(super) fn forward_children<'a>(
        &self,
        doc: &'a PenDocument,
        budget: &mut super::forward_structure::Budget,
    ) -> Option<super::forward_structure::Children<'a>> {
        use super::forward_structure::{node_ids, Children};
        self.validate(doc).ok()?;
        let (nodes, _) = self.parent.list(doc, &self.scope).ok()?;
        let mut before = node_ids(nodes)?;
        if self.wrapped {
            before.remove(self.index);
            for (position, id) in self.positions.iter().zip(&self.members) {
                if *position > before.len() {
                    return None;
                }
                before.insert(*position, id.clone());
            }
        } else {
            let positions = self
                .positions
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>();
            before = before
                .into_iter()
                .enumerate()
                .filter_map(|(at, id)| (!positions.contains(&at)).then_some(id))
                .collect();
            if self.index > before.len() {
                return None;
            }
            before.insert(self.index, self.header.id_str().to_owned());
        }
        Children::capture(doc, &self.scope, &self.parent.path, before, true, budget)
    }
}

#[path = "topology_wrap/canonical_delta.rs"]
mod canonical_delta;
