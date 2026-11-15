//! Project the actual reverse ownership move, without moving/cloning its body.
use super::super::canonical_deltas::{self as delta, ids};
use super::*;
use crate::{CanonicalSplice, PenNodeExt};
impl MovePatch {
    pub(in crate::history::scoped) fn canonical_deltas<'a>(
        &'a self,
        doc: &'a PenDocument,
    ) -> Option<Vec<CanonicalSplice<'a>>> {
        self.validate(doc).ok()?;
        let (source, _) = self.from.list(doc, &self.scope).ok()?;
        let (target, _) = self.to.list(doc, &self.scope).ok()?;
        let moved = source.get(self.from_index)?.id_str();
        let mut from_before = ids(source.iter().map(PenNodeExt::id_str))?;
        from_before.remove(self.from_index);
        if source.len() > delta::MAX_IDS || target.len() > delta::MAX_IDS {
            return None;
        }
        let reuse = source
            .iter()
            .chain(target)
            .map(PenNodeExt::id_str)
            .collect::<Vec<_>>();
        if self.from.path == self.to.path {
            if self.to_index > from_before.len() {
                return None;
            }
            from_before.insert(self.to_index, moved);
            return delta::list(
                doc,
                &self.scope,
                &self.from.path,
                from_before,
                true,
                reuse,
                &std::collections::HashSet::new(),
            )
            .map(|d| vec![d]);
        }
        let mut to_before = ids(target.iter().map(PenNodeExt::id_str))?;
        if self.to_index > to_before.len() {
            return None;
        }
        to_before.insert(self.to_index, moved);
        // Completed patch is the reverse: its current source was the forward
        // destination. Both original list projections are staged atomically.
        Some(vec![
            delta::list(
                doc,
                &self.scope,
                &self.from.path,
                from_before,
                self.restore_from_present,
                reuse.clone(),
                &std::collections::HashSet::new(),
            )?,
            delta::list(
                doc,
                &self.scope,
                &self.to.path,
                to_before,
                true,
                reuse,
                &std::collections::HashSet::new(),
            )?,
        ])
    }
}
