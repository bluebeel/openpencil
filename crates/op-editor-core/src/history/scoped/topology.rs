//! Trusted topology transactions retain identities and small wrapper metadata.
use super::{
    patch,
    topology_move::{MovePatch, PendingMove},
    topology_parent::Location,
    topology_wrap::{PendingWrap, WrapPatch},
    ScopedHistoryError,
};
use crate::{EditorCommand as C, EditorState, NodeId, SelectionState, SharedNode};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TopologyRequest {
    Move {
        node_id: NodeId,
        parent: NodeId,
        page: Option<String>,
        index: Option<usize>,
    },
    Reorder {
        anchor: NodeId,
        direction: crate::walkers::ReorderDirection,
    },
    Group(SelectionState),
    Ungroup(SelectionState),
}
impl TopologyRequest {
    pub(super) fn matches(&self, state: &EditorState, command: &C) -> bool {
        match (self, command) {
            (
                Self::Move {
                    node_id,
                    parent,
                    page,
                    index,
                },
                C::MoveNode {
                    node_id: current,
                    target_parent,
                    page_id,
                    index: at,
                },
            ) => node_id == current && parent == target_parent && page == page_id && index == at,
            (Self::Reorder { anchor, direction }, C::ReorderSelected { direction: current }) => {
                anchor == &state.selection.anchor && direction == current
            }
            (Self::Group(selection), C::GroupSelected)
            | (Self::Ungroup(selection), C::UngroupSelected) => selection == &state.selection,
            _ => false,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TopologyKind {
    Move {
        from: Location,
        to: Location,
        node_id: String,
        no_op_is_success: bool,
    },
    Group {
        positions: Box<[usize]>,
    },
    Ungroup {
        index: usize,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TopologySpec {
    pub request: TopologyRequest,
    pub kind: TopologyKind,
    pub forecast: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum TopologyPatch {
    Move(MovePatch),
    Wrap(WrapPatch),
}
impl TopologyPatch {
    pub(super) fn roots(&self) -> &[Arc<SharedNode>] {
        match self {
            Self::Move(_) => &[],
            Self::Wrap(patch) => patch.roots(),
        }
    }
    pub(super) fn cohort_bound(
        &self,
        doc: &crate::PenDocument,
        index: &super::identity::Index,
        bound: &mut super::cohort::Bound,
    ) -> Option<()> {
        match self {
            Self::Move(patch) => patch.cohort_bound(doc, index, bound),
            Self::Wrap(patch) => patch.cohort_bound(doc, index, bound),
        }
    }
    pub(super) fn bytes(&self) -> usize {
        match self {
            Self::Move(patch) => patch.bytes(),
            Self::Wrap(patch) => patch.bytes(),
        }
    }
    pub(super) fn validate(
        &self,
        state: &jian_ops_schema::PenDocument,
    ) -> Result<(), ScopedHistoryError> {
        match self {
            Self::Move(patch) => patch.validate(state),
            Self::Wrap(patch) => patch.validate(state),
        }
    }
    pub(super) fn exchange(&mut self, state: &mut jian_ops_schema::PenDocument) {
        match self {
            Self::Move(patch) => patch.exchange(state),
            Self::Wrap(patch) => patch.exchange(state),
        }
    }
    pub(super) fn want_identities(&self, wanted: &mut super::identity::Wanted) {
        match self {
            Self::Move(patch) => patch.want_identities(wanted),
            Self::Wrap(patch) => patch.want_identities(wanted),
        }
    }
    pub(super) fn rebase_identity(
        &mut self,
        index: &super::identity::Index,
    ) -> Result<(), ScopedHistoryError> {
        match self {
            Self::Move(patch) => patch.rebase_identity(index),
            Self::Wrap(patch) => patch.rebase_identity(index),
        }
    }
    pub(super) fn conflicts_splice(&self, splice: &super::splice::SplicePatch) -> bool {
        match self {
            Self::Move(patch) => patch.conflicts_splice(splice),
            Self::Wrap(patch) => patch.conflicts_splice(splice),
        }
    }
    pub(super) fn conflicts_node(&self, node: &super::patch::NodePatch) -> bool {
        match self {
            Self::Move(patch) => patch.conflicts_node(node),
            Self::Wrap(patch) => patch.conflicts_node(node),
        }
    }
    pub(super) fn actor_regions(&self) -> [Option<super::actor_claim::Region<'_>>; 2] {
        match self {
            Self::Move(patch) => patch.actor_regions(),
            Self::Wrap(patch) => patch.actor_regions(),
        }
    }
    pub(super) fn rebase(&mut self, reorders: &patch::Reorders) {
        match self {
            Self::Move(patch) => patch.rebase(reorders),
            Self::Wrap(patch) => patch.rebase(reorders),
        }
    }
}
#[derive(Debug)]
pub(super) enum PendingTopology {
    Move {
        pending: PendingMove,
        no_op_is_success: bool,
    },
    Wrap(PendingWrap),
}
impl PendingTopology {
    pub(super) fn capture(
        state: &EditorState,
        scope: patch::Scope,
        parent: Box<[usize]>,
        spec: &TopologySpec,
    ) -> Result<Self, ScopedHistoryError> {
        match &spec.kind {
            TopologyKind::Move {
                from,
                to,
                node_id,
                no_op_is_success,
            } => Ok(Self::Move {
                pending: PendingMove::capture(
                    &state.doc,
                    scope,
                    from.clone(),
                    to.clone(),
                    node_id,
                )?,
                no_op_is_success: *no_op_is_success,
            }),
            TopologyKind::Group { positions } => {
                PendingWrap::capture_group(&state.doc, scope, parent, positions.clone())
                    .map(Self::Wrap)
            }
            TopologyKind::Ungroup { index } => {
                PendingWrap::capture_ungroup(&state.doc, scope, parent, *index).map(Self::Wrap)
            }
        }
    }
    pub(super) fn matches(&self, state: &EditorState) -> bool {
        match self {
            Self::Move { pending, .. } => pending.matches(&state.doc),
            Self::Wrap(pending) => pending.matches(&state.doc),
        }
    }
    pub(super) fn apply_direct(&mut self, state: &mut EditorState) -> bool {
        let applied = match self {
            Self::Move {
                pending,
                no_op_is_success,
            } => pending.apply_direct(&mut state.doc, *no_op_is_success),
            Self::Wrap(pending) => pending.apply_direct(state),
        };
        if applied {
            state.mark_document_changed();
        }
        applied
    }
    pub(super) fn compact(
        self,
        state: &EditorState,
    ) -> Result<Option<TopologyPatch>, ScopedHistoryError> {
        match self {
            Self::Move { pending, .. } => pending
                .compact(&state.doc)
                .map(|patch| patch.map(TopologyPatch::Move)),
            Self::Wrap(pending) => pending
                .compact(&state.doc)
                .map(|patch| patch.map(TopologyPatch::Wrap)),
        }
    }
    pub(super) fn restore(self, state: &mut EditorState) -> Result<(), ScopedHistoryError> {
        if let Some(mut patch) = self.compact(state)? {
            patch.exchange(&mut state.doc);
        }
        Ok(())
    }
}
