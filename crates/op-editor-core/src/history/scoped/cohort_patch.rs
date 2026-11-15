//! Patch-local borrowed receipt sources; no history mutation or command replay.
use super::super::{cohort::Bound, identity, patch};
use super::{Patch, Scope};
use crate::{PenDocument, PenNodeExt};
use std::collections::HashSet;

fn want(wanted: &mut identity::Wanted, scope: &Scope, id: Option<&str>) {
    if let Some(id) = id {
        wanted
            .entry(scope.clone())
            .or_default()
            .insert(id.to_owned());
    }
}
impl Patch {
    pub(in crate::history::scoped) fn want_cohort_identities(&self, wanted: &mut identity::Wanted) {
        match self {
            Self::Node(node) => want(wanted, &node.scope, Some(&node.expected_id)),
            Self::Forest(patch) => want(wanted, &patch.scope, patch.parent_id.as_deref()),
            Self::Order(patch) => want(wanted, &patch.scope, patch.parent_id.as_deref()),
            Self::Topology(patch) => patch.want_identities(wanted),
            _ => {
                for splice in self.splices() {
                    want(wanted, &splice.scope, splice.parent_id.as_deref());
                }
            }
        }
    }
    pub(in crate::history::scoped) fn cohort_bound(
        &self,
        doc: &PenDocument,
        index: &identity::Index,
        bound: &mut Bound,
    ) -> Option<()> {
        match self {
            Self::Node(node) => {
                let path = index.path(&node.scope, &node.expected_id).ok()?;
                let live = patch::node(patch::roots(doc, &node.scope).ok()?, &path)?;
                bound.live(live, node.metadata_only)
            }
            Self::Forest(patch) => {
                for node in bound.parent(doc, &patch.scope, patch.parent_id.as_deref(), index)? {
                    bound.live(node, false)?;
                }
                Some(())
            }
            Self::Order(patch) => {
                let retained: HashSet<_> = patch.old_order.iter().map(String::as_str).collect();
                for node in bound.parent(doc, &patch.scope, patch.parent_id.as_deref(), index)? {
                    if !retained.contains(node.id_str()) {
                        bound.live(node, false)?;
                    }
                }
                Some(())
            }
            Self::Topology(patch) => patch.cohort_bound(doc, index, bound),
            _ => {
                for splice in self.splices() {
                    let nodes =
                        bound.parent(doc, &splice.scope, splice.parent_id.as_deref(), index)?;
                    let expected: HashSet<_> =
                        splice.expected_ids.iter().map(String::as_str).collect();
                    if expected.len() != splice.expected_ids.len() {
                        return None;
                    }
                    let mut needed = expected.clone();
                    let mut seen = HashSet::new();
                    for node in nodes {
                        if expected.contains(node.id_str()) {
                            if !seen.insert(node.id_str()) {
                                return None;
                            }
                            needed.remove(node.id_str());
                            bound.live(node, false)?;
                        }
                    }
                    if !needed.is_empty() {
                        return None;
                    }
                }
                Some(())
            }
        }
    }
}
