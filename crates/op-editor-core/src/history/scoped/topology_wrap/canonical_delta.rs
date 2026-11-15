//! Wrapping changes ownership only; members retain their complete objects.
use super::super::canonical_deltas::{self as delta, ids};
use super::*;
impl WrapPatch {
    pub(in crate::history::scoped) fn canonical_deltas<'a>(
        &'a self,
        doc: &'a PenDocument,
    ) -> Option<crate::CanonicalSplice<'a>> {
        if self.members.len() > delta::MAX_IDS {
            return None;
        }
        self.validate(doc).ok()?;
        let (nodes, _) = self.parent.list(doc, &self.scope).ok()?;
        let mut before = ids(nodes.iter().map(PenNodeExt::id_str))?;
        let mut reuse = Vec::new();
        if self.wrapped {
            before.remove(self.index);
            for (position, id) in self.positions.iter().zip(&self.members) {
                if *position > before.len() {
                    return None;
                }
                before.insert(*position, id);
            }
            // New wrapper is serialized shallowly; original member bodies can
            // be referenced even though now nested under that new wrapper.
            reuse.extend(before.iter().copied());
        } else {
            let positions = self
                .positions
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>();
            before = before
                .into_iter()
                .enumerate()
                .filter_map(|(i, id)| (!positions.contains(&i)).then_some(id))
                .collect();
            if self.index > before.len() {
                return None;
            }
            before.insert(self.index, self.header.id_str());
            reuse.extend(nodes.iter().map(PenNodeExt::id_str));
        }
        delta::list(
            doc,
            &self.scope,
            &self.parent.path,
            before,
            true,
            reuse,
            &std::collections::HashSet::new(),
        )
    }
}
