//! An index moves out of the lexical cache during prepared clone commit. Only
//! real plan gaps and the accepted canonical payload can publish it again.
use super::{children_at, Cache, Edit, Index, Order, Pending, Scope, C};
use crate::mutators::clone_splice::LocationGap;
use crate::{EditorState, PenNodeExt};
use std::collections::BTreeMap;
pub(super) fn supports(command: &C) -> bool {
    matches!(
        command,
        C::DuplicateSelected { .. } | C::PasteClipboard { .. }
    )
}
pub(super) fn pending(index: &Index, state: &EditorState, command: &C) -> Option<Pending> {
    let scope = state.command_lookup_active_scope();
    let root = index.root(&scope)?;
    Some(Pending {
        scope,
        parent: root,
        len: index.entries[root].children.len(),
        revision: state.document_revision(),
        command: Some(Box::new(command.clone())),
        edit: Edit::Clones,
    })
}
#[derive(Debug)]
struct List {
    parent: usize,
    gaps: Vec<(usize, usize)>,
}
pub(crate) struct CloneLocations {
    index: Index,
    scope: Scope,
    generation: u64,
    revision: u64,
    lists: Vec<List>,
}
impl CloneLocations {
    fn prepare(index: Index, state: &EditorState, gaps: Vec<LocationGap>) -> Option<Self> {
        if gaps.len() > 4096 {
            return None;
        }
        let scope = state.command_lookup_active_scope();
        let root = index.root(&scope)?;
        let mut lists = BTreeMap::<Box<[usize]>, Vec<(usize, usize)>>::new();
        for gap in gaps {
            if gap.count == 0 {
                continue;
            }
            lists
                .entry(gap.parent)
                .or_default()
                .push((gap.at, gap.count));
        }
        let mut plans = Vec::with_capacity(lists.len());
        // Lexical route order processes ancestors before descendants. Original
        // child tokens then resolve their current final paths after each merge.
        for (path, mut gaps) in lists {
            let mut parent = root;
            for slot in path.iter() {
                parent = *index.entries[parent].children.get(*slot)?;
            }
            gaps.sort_unstable_by_key(|(at, _)| *at);
            let nodes = children_at(&state.doc, &scope, &path)?;
            let old = &index.entries[parent].children;
            if nodes.len() != old.len()
                || nodes
                    .iter()
                    .zip(old)
                    .any(|(node, key)| node.id_str() != index.entries[*key].id.as_ref())
                || gaps.iter().any(|(at, _)| *at > old.len())
                || gaps.windows(2).any(|gaps| gaps[0].0 == gaps[1].0)
            {
                return None;
            }
            plans.push(List { parent, gaps });
        }
        Some(Self {
            index,
            scope,
            generation: state.document_generation(),
            revision: state.document_revision(),
            lists: plans,
        })
    }
    pub(crate) fn finish(mut self, state: &EditorState, applied: bool) {
        let seed = &state.command_id_seed;
        if !seed.entered
            || !seed.valid
            || seed.generation != self.generation
            || state.document_generation() != self.generation
            || state.document_revision() != self.revision
            || state.command_lookup_active_scope() != self.scope
        {
            return;
        }
        if applied {
            for list in &self.lists {
                if self
                    .index
                    .merge_clone_list(state, &self.scope, list)
                    .is_none()
                {
                    return;
                }
            }
        }
        if let Ok(mut cache) = seed.lookup.lock() {
            // No callback may replace/observe the cache while the real prepared
            // plan mutates canonical Vecs. A changed cache is never overwritten.
            if matches!(*cache, Cache::Invalid) {
                *cache = Cache::Ready(self.index);
            }
        }
    }
}
impl Index {
    fn merge_clone_list(&mut self, state: &EditorState, scope: &Scope, list: &List) -> Option<()> {
        let nodes = children_at(&state.doc, scope, &self.path(list.parent))?;
        let old_len = self.entries[list.parent].children.len();
        let added = list
            .gaps
            .iter()
            .try_fold(0usize, |total, (_, count)| total.checked_add(*count))?;
        if nodes.len() != old_len.checked_add(added)? {
            return None;
        }
        let mut gap = 0;
        let mut added_before = 0;
        for slot in 0..old_len {
            while list.gaps.get(gap).is_some_and(|(at, _)| *at <= slot) {
                added_before += list.gaps[gap].1;
                gap += 1;
            }
            let key = self.entries[list.parent].children[slot];
            if nodes[slot + added_before].id_str() != self.entries[key].id.as_ref() {
                return None;
            }
        }
        let mut ranges = Vec::with_capacity(list.gaps.len());
        let mut before = 0;
        for &(at, count) in &list.gaps {
            let start = at.checked_add(before)?;
            ranges.push((at, start, count));
            before = before.checked_add(count)?;
        }
        for &(_, start, count) in &ranges {
            self.append(list.parent, &nodes[start..start + count]);
        }
        let mut destinations = vec![0; nodes.len()];
        let (mut old, mut new, mut output) = (0, old_len, 0);
        for &(at, _, count) in &ranges {
            while old < at {
                destinations[old] = output;
                old += 1;
                output += 1;
            }
            for _ in 0..count {
                destinations[new] = output;
                new += 1;
                output += 1;
            }
        }
        while old < old_len {
            destinations[old] = output;
            old += 1;
            output += 1;
        }
        let children = &mut self.entries[list.parent].children;
        for slot in 0..destinations.len() {
            while destinations[slot] != slot {
                let target = destinations[slot];
                children.swap(slot, target);
                destinations.swap(slot, target);
            }
        }
        for slot in 0..children.len() {
            let key = children[slot];
            // Slots are assigned after the mutable list borrow ends below.
            destinations[slot] = key;
        }
        for (slot, key) in destinations.into_iter().enumerate() {
            self.entries[key].slot = slot;
        }
        Some(())
    }
}
impl EditorState {
    pub(crate) fn prepare_clone_locations(
        &self,
        gaps: impl FnOnce() -> Vec<LocationGap>,
    ) -> Option<CloneLocations> {
        let seed = &self.command_id_seed;
        if !seed.entered || !seed.valid || seed.generation != self.document_generation() {
            return None;
        }
        let mut cache = seed.lookup.lock().ok()?;
        if !cache.ready(self) {
            return None;
        }
        let prior = std::mem::replace(&mut *cache, Cache::Invalid);
        let index = match prior {
            Cache::Ready(index) | Cache::Pending(index, _) => index,
            _ => return None,
        };
        CloneLocations::prepare(index, self, gaps())
    }
    pub(crate) fn command_lookup_variants(
        &self,
        scope: &Scope,
        id: &str,
    ) -> Option<(Option<Box<[usize]>>, Option<Box<[usize]>>, usize)> {
        let seed = &self.command_id_seed;
        if !seed.entered || !seed.valid || seed.generation != self.document_generation() {
            return None;
        }
        let mut cache = seed.lookup.lock().ok()?;
        if !cache.ready(self) {
            return None;
        }
        let index = match &*cache {
            Cache::Ready(index) | Cache::Pending(index, _) => index,
            _ => return None,
        };
        let root = index.root(scope)?;
        let count = index.ids.get(id).map_or(0, |keys| {
            keys.keys()
                .iter()
                .filter(|key| index.entries[**key].root == root)
                .count()
        });
        Some((
            index
                .select(root, id, Order::Preorder)
                .map(|key| index.path(key)),
            index
                .select(root, id, Order::SiblingFirst)
                .map(|key| index.path(key)),
            count,
        ))
    }
}
