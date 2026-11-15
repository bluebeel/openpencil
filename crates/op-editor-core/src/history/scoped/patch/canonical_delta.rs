//! Exact opposite-side order already retained by ordinary scoped compaction.
use super::super::canonical_deltas::{self as delta, ids};
use super::*;
impl OrderPatch {
    pub(in crate::history::scoped) fn canonical_delta<'a>(
        &'a self,
        doc: &'a PenDocument,
    ) -> Option<crate::CanonicalSplice<'a>> {
        let nodes = child_list(doc, &self.scope, &self.parent, self.parent_id.as_deref()).ok()?;
        if nodes.len() != self.expected_ids.len()
            || nodes
                .iter()
                .map(PenNodeExt::id_str)
                .ne(self.expected_ids.iter().map(String::as_str))
        {
            return None;
        }
        // Compaction recursively emitted every changed common node; untouched
        // common bodies can reuse source bytes after those companion deltas.
        let current = nodes
            .iter()
            .map(PenNodeExt::id_str)
            .collect::<std::collections::HashSet<_>>();
        let reuse = self
            .old_order
            .iter()
            .map(String::as_str)
            .filter(|id| current.contains(id))
            .collect();
        delta::list(
            doc,
            &self.scope,
            &self.parent,
            ids(self.old_order.iter().map(String::as_str))?,
            true,
            reuse,
            &std::collections::HashSet::new(),
        )
    }
}
impl NodePatch {
    pub(in crate::history::scoped) fn canonical_replacement<'a>(
        &'a self,
        doc: &'a PenDocument,
    ) -> Option<crate::CanonicalSplice<'a>> {
        if self.metadata_only {
            return None;
        }
        let (at, parent) = self.path.split_last()?;
        let (nodes, present, _) = delta::current(doc, &self.scope, parent)?;
        if nodes.get(*at)?.id_str() != self.expected_id {
            return None;
        }
        let mut before = ids(nodes.iter().map(PenNodeExt::id_str))?;
        before[*at] = self.old.id_str();
        let changed = [self.old.id_str(), self.expected_id.as_str()]
            .into_iter()
            .collect();
        delta::list(
            doc,
            &self.scope,
            parent,
            before,
            present,
            Vec::new(),
            &changed,
        )
    }
}
