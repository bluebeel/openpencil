//! Gap insertion plans keep original sibling coordinates stable until commit.
use super::lookup;
use crate::{NodeId, PenNodeExt};
use jian_ops_schema::node::PenNode;
use std::collections::{BTreeMap, HashMap, HashSet};
#[derive(Clone)]
pub(super) enum Source {
    Original(Box<[usize]>),
    Owned { root: usize, path: Box<[usize]> },
}
#[derive(Clone, Copy)]
struct Chain {
    first: usize,
    last: usize,
    len: usize,
}
struct Pending {
    node: Option<PenNode>,
    list: usize,
    gap: usize,
    prev: Option<usize>,
    next: Option<usize>,
}
struct List {
    path: Box<[usize]>,
    original_len: usize,
    count: usize,
    gaps: BTreeMap<usize, Chain>,
    fenwick: Vec<usize>,
}
impl List {
    fn before(&self, gap: usize) -> usize {
        let mut i = gap;
        let mut sum = 0;
        while i > 0 {
            sum += self.fenwick[i];
            i &= i - 1;
        }
        sum
    }
    fn added(&mut self, gap: usize) {
        let mut i = gap + 1;
        while i < self.fenwick.len() {
            self.fenwick[i] += 1;
            i += i & (!i + 1);
        }
        self.count += 1;
    }
    fn locate(&self, position: usize) -> (usize, usize) {
        let position = position.min(self.original_len + self.count);
        let mut low = 0;
        let mut high = self.original_len;
        while low < high {
            let middle = low + (high - low + 1) / 2;
            if middle + self.before(middle) <= position {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        (low, position - low - self.before(low))
    }
}
pub(super) struct Plan {
    wanted: HashSet<String>,
    owned: HashMap<String, Source>,
    paths: BTreeMap<Box<[usize]>, usize>,
    lists: Vec<List>,
    pending: Vec<Pending>,
}
impl Plan {
    pub(super) fn new(wanted: HashSet<String>) -> Self {
        Self {
            wanted,
            owned: HashMap::new(),
            paths: BTreeMap::new(),
            lists: Vec::new(),
            pending: Vec::new(),
        }
    }
    pub(super) fn source(&self, index: &lookup::Index, id: &str) -> Option<Source> {
        index
            .original
            .get(id)
            .map(|source| Source::Original(source.sibling_first.clone()))
            .or_else(|| self.owned.get(id).cloned())
    }
    pub(super) fn node<'a>(&'a self, roots: &'a [PenNode], source: &Source) -> &'a PenNode {
        match source {
            Source::Original(path) => lookup::at(roots, path).unwrap(),
            Source::Owned { root, path } => {
                lookup::relative(self.pending[*root].node.as_ref().unwrap(), path).unwrap()
            }
        }
    }
    pub(super) fn list(&mut self, roots: &[PenNode], path: &[usize]) -> usize {
        if let Some(id) = self.paths.get(path) {
            return *id;
        }
        let nodes = if path.is_empty() {
            roots
        } else {
            lookup::at(roots, path)
                .unwrap()
                .children()
                .unwrap()
                .as_slice()
        };
        let id = self.lists.len();
        self.paths.insert(path.into(), id);
        self.lists.push(List {
            path: path.into(),
            original_len: nodes.len(),
            count: 0,
            gaps: BTreeMap::new(),
            fenwick: vec![0; nodes.len() + 2],
        });
        id
    }
    fn register(&mut self, root: usize, base: &[usize], node: &PenNode) {
        if self.wanted.is_empty() {
            return;
        }
        if self.wanted.contains(node.id_str()) {
            self.owned.insert(
                node.id_str().into(),
                Source::Owned {
                    root,
                    path: base.into(),
                },
            );
        }
        lookup::walk(
            node.children().map_or(&[][..], Vec::as_slice),
            |child, path| {
                if self.wanted.contains(child.id_str()) {
                    let mut full = base.to_vec();
                    full.extend(path);
                    self.owned.insert(
                        child.id_str().into(),
                        Source::Owned {
                            root,
                            path: full.into_boxed_slice(),
                        },
                    );
                }
            },
        );
    }
    fn place(&mut self, list: usize, gap: usize, previous: Option<usize>, node: PenNode) -> usize {
        let old = self.lists[list].gaps.get(&gap).copied();
        let next =
            previous.map_or_else(|| old.map(|chain| chain.first), |id| self.pending[id].next);
        let id = self.pending.len();
        self.register(id, &[], &node);
        self.pending.push(Pending {
            node: Some(node),
            list,
            gap,
            prev: previous,
            next,
        });
        if let Some(previous) = previous {
            self.pending[previous].next = Some(id);
        }
        if let Some(next) = next {
            self.pending[next].prev = Some(id);
        }
        let chain = self.lists[list].gaps.entry(gap).or_insert(Chain {
            first: id,
            last: id,
            len: 0,
        });
        if previous.is_none() {
            chain.first = id;
        }
        if next.is_none() {
            chain.last = id;
        }
        chain.len += 1;
        self.lists[list].added(gap);
        id
    }
    pub(super) fn insert_original_after(
        &mut self,
        roots: &[PenNode],
        path: &[usize],
        node: PenNode,
    ) {
        let (&index, parent) = path.split_last().unwrap();
        let list = self.list(roots, parent);
        self.place(list, index + 1, None, node);
    }
    pub(super) fn insert_numeric(&mut self, list: usize, position: Option<usize>, node: PenNode) {
        let length = self.lists[list].original_len + self.lists[list].count;
        let (gap, offset) = self.lists[list].locate(position.unwrap_or(length));
        let chain = self.lists[list].gaps.get(&gap).copied();
        let previous = if offset == 0 {
            None
        } else {
            let chain = chain.unwrap();
            if offset <= chain.len / 2 {
                let mut at = chain.first;
                for _ in 1..offset {
                    at = self.pending[at].next.unwrap();
                }
                Some(at)
            } else {
                let mut at = chain.last;
                for _ in offset..chain.len {
                    at = self.pending[at].prev.unwrap();
                }
                Some(at)
            }
        };
        self.place(list, gap, previous, node);
    }
    pub(super) fn insert_owned_after(&mut self, root: usize, path: &[usize], node: PenNode) {
        if path.is_empty() {
            let list = self.pending[root].list;
            let gap = self.pending[root].gap;
            self.place(list, gap, Some(root), node);
            return;
        }
        let (&index, parent) = path.split_last().unwrap();
        let at = index + 1;
        for source in self.owned.values_mut() {
            if let Source::Owned { root: owner, path } = source {
                if *owner == root
                    && path.len() > parent.len()
                    && path.starts_with(parent)
                    && path[parent.len()] >= at
                {
                    path[parent.len()] += 1;
                }
            }
        }
        let mut inserted = parent.to_vec();
        inserted.push(at);
        self.register(root, &inserted, &node);
        let target = self.pending[root].node.as_mut().unwrap();
        let parent = lookup::relative_mut(target, parent).unwrap();
        parent.children_mut().unwrap().insert(at, node);
    }
    pub(super) fn gap_nodes(
        &self,
        path: &[usize],
        gap: usize,
        mut visit: impl FnMut(&PenNode) -> Result<(), crate::IdAllocError>,
    ) -> Result<(), crate::IdAllocError> {
        if let Some(chain) = self
            .paths
            .get(path)
            .and_then(|list| self.lists[*list].gaps.get(&gap))
        {
            let mut next = Some(chain.first);
            while let Some(id) = next {
                visit(self.pending[id].node.as_ref().unwrap())?;
                next = self.pending[id].next;
            }
        }
        Ok(())
    }
    pub(super) fn extra_children(&self, path: &[usize]) -> usize {
        self.paths
            .get(path)
            .map_or(0, |list| self.lists[*list].count)
    }
    pub(super) fn commit(mut self, roots: &mut Vec<PenNode>) {
        // Deeper lists commit before ancestors can shift their original paths.
        for list in self.paths.values().rev().copied() {
            let spec = &self.lists[list];
            let nodes = if spec.path.is_empty() {
                &mut *roots
            } else {
                lookup::at_mut(roots, &spec.path)
                    .unwrap()
                    .children_mut()
                    .unwrap()
            };
            let old_len = nodes.len();
            debug_assert_eq!(old_len, spec.original_len);
            let mut destinations = vec![0; old_len + spec.count];
            let mut append = Vec::with_capacity(spec.count);
            let mut original = 0;
            let mut cursor = 0;
            for (gap, chain) in &spec.gaps {
                while original < *gap {
                    destinations[original] = cursor;
                    original += 1;
                    cursor += 1;
                }
                let mut next = Some(chain.first);
                while let Some(id) = next {
                    destinations[old_len + append.len()] = cursor;
                    cursor += 1;
                    append.push(id);
                    next = self.pending[id].next;
                }
            }
            while original < old_len {
                destinations[original] = cursor;
                original += 1;
                cursor += 1;
            }
            nodes.reserve_exact(spec.count);
            for id in append {
                nodes.push(self.pending[id].node.take().unwrap());
            }
            // One bounded in-place stable permutation. Only the usize table is
            // duplicated; unrelated PenNode payloads are moved, never cloned.
            for index in 0..destinations.len() {
                while destinations[index] != index {
                    let at = destinations[index];
                    nodes.swap(index, at);
                    destinations.swap(index, at);
                }
            }
        }
    }
}
