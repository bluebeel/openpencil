//! Identity guards for scoped insertion/deletion history. Sibling counts and
//! numeric slots are capture-time hints, never selective undo authorities.
use super::{
    identity,
    patch::{self, Patch, Scope},
    splice::SplicePatch,
    splice_order::Anchors,
    ScopedHistoryError,
};
use crate::{PenDocument, PenNodeExt};
use std::{collections::HashSet, sync::Arc};
impl SplicePatch {
    pub(super) fn contains_changed(&self, id: &str) -> bool {
        self.changed_ids
            .binary_search_by(|known| known.as_str().cmp(id))
            .is_ok()
    }
    pub(super) fn same_parent(&self, other: &Self) -> bool {
        self.scope == other.scope && self.parent_id == other.parent_id
    }
    pub(super) fn anchors(&self, required: bool) -> Anchors {
        Anchors {
            ids: self.logical_ids.clone(),
            left: self.left.clone(),
            right: self.right.clone(),
            required,
        }
    }
    pub(super) fn current<'a>(
        &self,
        doc: &'a PenDocument,
    ) -> Result<(&'a [jian_ops_schema::node::PenNode], bool), ScopedHistoryError> {
        let roots = patch::roots(doc, &self.scope)?;
        if self.parent_id.is_none() {
            return Ok((roots, true));
        }
        let parent = patch::node(roots, &self.parent).ok_or(ScopedHistoryError::NodePathChanged)?;
        if Some(parent.id_str()) != self.parent_id.as_deref()
            || Some(std::mem::discriminant(parent)) != self.parent_kind
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        Ok((
            parent.children().map_or(&[][..], Vec::as_slice),
            parent.children().is_some(),
        ))
    }
    pub(super) fn validate_identity(&self, doc: &PenDocument) -> Result<(), ScopedHistoryError> {
        let (nodes, present) = self.current(doc)?;
        if present != self.expected_present {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let wanted: HashSet<_> = self.expected_ids.iter().map(String::as_str).collect();
        let mut seen = HashSet::new();
        let mut order = Vec::new();
        for node in nodes {
            if wanted.contains(node.id_str()) {
                if !seen.insert(node.id_str()) {
                    return Err(ScopedHistoryError::NodePathChanged);
                }
                order.push(node.id_str());
            }
        }
        if order
            .iter()
            .copied()
            .ne(self.expected_ids.iter().map(String::as_str))
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        Ok(())
    }
    pub(super) fn from_trace(
        scope: Scope,
        trace: crate::mutators::clone_splice::Trace,
        index: &identity::Index,
        total: usize,
        positions: &std::collections::HashMap<String, Option<usize>>,
    ) -> Result<Self, ScopedHistoryError> {
        let parent = match &trace.parent_id {
            Some(id) => index.path(&scope, id)?,
            None => Box::new([]),
        };
        let slots = trace
            .roots
            .iter()
            .map(|id| {
                positions
                    .get(id)
                    .copied()
                    .flatten()
                    .ok_or(ScopedHistoryError::NodePathChanged)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let at = *slots.first().ok_or(ScopedHistoryError::NodePathChanged)?;
        if slots.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let patch = Self {
            scope,
            parent,
            parent_id: trace.parent_id,
            parent_kind: trace.parent_kind,
            index: at,
            expected_total: total,
            expected_present: true,
            expected_ids: trace.roots.clone(),
            logical_ids: trace.roots,
            changed_ids: Arc::new(trace.changed),
            left: trace.left,
            right: trace.right,
            old_present: trace.present,
            origin_absent: !trace.present,
            old: Vec::new(),
            recent: Vec::new(),
            sealed: true,
        };
        Ok(patch)
    }
    pub(super) fn conflicts_splice(&self, other: &Self) -> bool {
        if self.scope != other.scope {
            return false;
        }
        if self.changed_ids.iter().any(|id| other.contains_changed(id)) {
            return true;
        }
        if other
            .parent_id
            .as_deref()
            .is_some_and(|id| self.contains_changed(id))
            || self
                .parent_id
                .as_deref()
                .is_some_and(|id| other.contains_changed(id))
        {
            return true;
        }
        self.same_parent(other) && (self.origin_absent || other.origin_absent)
    }
    pub(super) fn conflicts_patch(&self, other: &Patch) -> bool {
        if let Patch::Node(node) = other {
            if node.scope != self.scope {
                return false;
            }
            if self.contains_changed(&node.expected_id) || self.contains_changed(node.old.id_str())
            {
                return true;
            }
            if self.origin_absent && self.parent_id.as_deref() == Some(node.expected_id.as_str()) {
                return true;
            }
            if !node.metadata_only {
                if self
                    .parent_id
                    .as_deref()
                    .is_some_and(|id| node.contains_affected(id))
                {
                    return true;
                }
                if self.changed_ids.iter().any(|id| node.contains_affected(id)) {
                    return true;
                }
            }
            return false;
        }
        if let Patch::Topology(topology) = other {
            return topology.conflicts_splice(self);
        }
        let mut splices = other.splices().peekable();
        if splices.peek().is_some() {
            return splices.any(|splice| self.conflicts_splice(splice));
        }
        // Ordering/grouping/opaque structural patches own the affected child
        // ordering. They retain their existing conservative overlap regions.
        other.regions().into_iter().flatten().any(|region| {
            region.scope == &self.scope
                && (region.path.starts_with(&self.parent) || self.parent.starts_with(region.path))
        })
    }
}
