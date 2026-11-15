//! Narrow forward append proof from an actual completed inverse. Rebased
//! history, insertion-before-sibling and absent child arrays remain fallback.
use super::*;
use crate::{CanonicalAppend, CanonicalChildTarget};
impl SplicePatch {
    pub(in crate::history::scoped) fn canonical_append<'a>(
        &'a self,
        doc: &'a PenDocument,
    ) -> Option<CanonicalAppend<'a>> {
        if !self.sealed
            || !self.old.is_empty()
            || !self.old_present
            || !self.expected_present
            || self.origin_absent
            || !self.recent.is_empty()
            || self.right.is_some()
            || self.expected_ids.is_empty()
            || self.expected_ids != self.logical_ids
        {
            return None;
        }
        let roots = patch::roots(doc, &self.scope).ok()?;
        let (children, target) = if self.parent.is_empty() {
            if self.parent_id.is_some() {
                return None;
            }
            let target = match &self.scope {
                patch::Scope::Root => CanonicalChildTarget::Root,
                patch::Scope::Page { index, id } => {
                    CanonicalChildTarget::Page { index: *index, id }
                }
            };
            (roots, target)
        } else {
            let parent = patch::node(roots, &self.parent)?;
            if Some(parent.id_str()) != self.parent_id.as_deref()
                || Some(mem::discriminant(parent)) != self.parent_kind
            {
                return None;
            }
            let PenNode::Frame(frame) = parent else {
                return None;
            };
            (
                frame.children.as_ref()?.as_slice(),
                CanonicalChildTarget::Node {
                    id: parent.id_str(),
                },
            )
        };
        if children.len() != self.expected_total
            || self.index.checked_add(self.expected_ids.len())? != children.len()
            || self
                .index
                .checked_sub(1)
                .and_then(|i| children.get(i))
                .map(PenNodeExt::id_str)
                != self.left.as_deref()
        {
            return None;
        }
        let nodes = children.get(self.index..)?;
        if nodes
            .iter()
            .zip(&self.expected_ids)
            .any(|(node, id)| node.id_str() != id)
        {
            return None;
        }
        Some(CanonicalAppend {
            target,
            previous_len: self.index,
            previous_last: self.left.as_deref(),
            nodes,
        })
    }
}
