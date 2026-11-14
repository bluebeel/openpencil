//! Prepare every identity/ordering decision before the first canonical write.
use super::{
    identity,
    patch::{self, Patch, Scope},
    splice::SplicePatch,
    splice_order::{self, Anchors, Token},
    ScopedHistoryError,
};
use crate::{History, PenDocument, PenNodeExt, SharedNode};
use jian_ops_schema::node::PenNode;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
#[derive(Clone, PartialEq, Eq, Hash)]
struct ParentKey {
    scope: Scope,
    id: Option<String>,
}
struct Record {
    key: ParentKey,
    anchor: Anchors,
}
pub(super) struct Catalog(Vec<Record>);
impl Catalog {
    pub(super) fn new(history: &History, current: &[Patch]) -> Self {
        let mut records = Vec::new();
        for snapshot in history
            .past
            .iter()
            .chain(&history.future)
            .filter(|entry| !history.operation_conflicted(entry.history_tag().operation))
            .filter_map(|entry| entry.scoped_snapshot())
        {
            Self::append(&mut records, &snapshot.patches);
        }
        Self::append(&mut records, current);
        Self(records)
    }
    fn append(records: &mut Vec<Record>, patches: &[Patch]) {
        for splice in patches.iter().flat_map(Patch::splices) {
            records.push(Record {
                key: ParentKey {
                    scope: splice.scope.clone(),
                    id: splice.parent_id.clone(),
                },
                anchor: splice.anchors(false),
            });
        }
    }
    fn matching<'a>(&'a self, key: &'a ParentKey) -> impl Iterator<Item = Anchors> + 'a {
        self.0
            .iter()
            .filter(move |record| &record.key == key)
            .map(|record| record.anchor.clone())
    }
}
struct Group {
    patch: usize,
    splice: usize,
    expected: Vec<String>,
    restore: Vec<String>,
    present: bool,
    recent: Vec<Anchors>,
}
#[path = "splice_metadata.rs"]
mod metadata;
struct ParentPlan {
    scope: Scope,
    path: Box<[usize]>,
    groups: Vec<Group>,
    order: Vec<Token>,
    restore: Vec<Arc<SharedNode>>,
    remaining: usize,
    absence: bool,
    relocation: Vec<Option<usize>>,
}
pub(super) struct Prepared {
    parents: Vec<ParentPlan>,
    metadata: Vec<(usize, Box<[usize]>)>,
}
fn want(index: &mut identity::Wanted, scope: &Scope, id: &Option<String>) {
    if let Some(id) = id {
        index.entry(scope.clone()).or_default().insert(id.clone());
    }
}
impl Prepared {
    pub(super) fn capture(
        doc: &PenDocument,
        patches: &[Patch],
        catalog: &Catalog,
        check_collisions: bool,
    ) -> Result<Option<Self>, ScopedHistoryError> {
        if !patches.iter().any(|patch| patch.splices().next().is_some()) {
            return Ok(None);
        }
        if patches.iter().any(|patch| {
            patch.splices().next().is_none()
                && !matches!(patch,Patch::Node(node) if node.metadata_only)
        }) {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let mut wanted = identity::Wanted::new();
        for splice in patches.iter().flat_map(Patch::splices) {
            want(&mut wanted, &splice.scope, &splice.parent_id);
        }
        for patch in patches {
            if let Patch::Node(node) = patch {
                wanted
                    .entry(node.scope.clone())
                    .or_default()
                    .insert(node.expected_id.clone());
            }
        }
        let index = identity::Index::new(doc, wanted)?;
        let mut grouped: HashMap<ParentKey, Vec<(usize, usize)>> = HashMap::new();
        for (patch, entry) in patches.iter().enumerate() {
            for (splice, item) in entry.splices().enumerate() {
                grouped
                    .entry(ParentKey {
                        scope: item.scope.clone(),
                        id: item.parent_id.clone(),
                    })
                    .or_default()
                    .push((patch, splice));
            }
        }
        let mut parents = Vec::new();
        let mut restoring_ids = HashSet::new();
        for (key, entries) in grouped {
            let path = match &key.id {
                Some(id) => index.path(&key.scope, id)?,
                None => Box::new([]),
            };
            let roots = patch::roots(doc, &key.scope)?;
            let (nodes, present) = if key.id.is_none() {
                (roots, true)
            } else {
                let parent =
                    patch::node(roots, &path).ok_or(ScopedHistoryError::NodePathChanged)?;
                (
                    parent.children().map_or(&[][..], Vec::as_slice),
                    parent.children().is_some(),
                )
            };
            let mut positions_by_id = HashMap::with_capacity(nodes.len());
            for (at, node) in nodes.iter().enumerate() {
                match positions_by_id.entry(node.id_str()) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(Some(at));
                    }
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        entry.insert(None);
                    }
                }
            }
            let mut groups = Vec::new();
            let mut removed = HashSet::new();
            let mut restore = Vec::new();
            let mut restores = Vec::new();
            let mut anchors = Vec::new();
            let mut absence = false;
            for (patch_at, splice_at) in entries {
                let splice = patches[patch_at].splice_at(splice_at);
                if let Some(parent) = patch::node(roots, &path).filter(|_| key.id.is_some()) {
                    if Some(std::mem::discriminant(parent)) != splice.parent_kind {
                        return Err(ScopedHistoryError::NodePathChanged);
                    }
                }
                if present != splice.expected_present {
                    return Err(ScopedHistoryError::NodePathChanged);
                }
                let positions = splice
                    .expected_ids
                    .iter()
                    .map(|id| {
                        positions_by_id
                            .get(id.as_str())
                            .copied()
                            .flatten()
                            .ok_or(ScopedHistoryError::NodePathChanged)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if positions.windows(2).any(|pair| pair[0] >= pair[1]) {
                    return Err(ScopedHistoryError::NodePathChanged);
                }
                let recent = positions
                    .iter()
                    .map(|at| Anchors {
                        ids: vec![nodes[*at].id_str().to_owned()],
                        left: at
                            .checked_sub(1)
                            .map(|left| nodes[left].id_str().to_owned()),
                        right: nodes.get(at + 1).map(|right| right.id_str().to_owned()),
                        required: false,
                    })
                    .collect::<Vec<_>>();
                for id in &splice.expected_ids {
                    if !removed.insert(id.clone()) {
                        return Err(ScopedHistoryError::NodePathChanged);
                    }
                }
                let ids = splice
                    .old
                    .iter()
                    .map(|node| node.id_str().to_owned())
                    .collect::<Vec<_>>();
                for node in &splice.old {
                    let mut pending = vec![node.as_ref()];
                    while let Some(node) = pending.pop() {
                        if !restoring_ids.insert(node.id_str().to_owned()) {
                            return Err(ScopedHistoryError::NodePathChanged);
                        }
                        if let Some(children) = node.children() {
                            pending.extend(children.iter().map(Arc::as_ref));
                        }
                    }
                }
                restores.extend(ids.iter().cloned());
                restore.extend(splice.old.iter().cloned());
                anchors.push(splice.anchors(!ids.is_empty()));
                anchors.extend(splice.recent.iter().cloned());
                absence |= !splice.old_present;
                groups.push(Group {
                    patch: patch_at,
                    splice: splice_at,
                    expected: splice.expected_ids.clone(),
                    restore: ids,
                    present,
                    recent,
                });
            }
            let remaining_ids = nodes
                .iter()
                .filter(|node| !removed.contains(node.id_str()))
                .map(|node| node.id_str().to_owned())
                .collect::<Vec<_>>();
            let order = if restores.is_empty() {
                (0..remaining_ids.len()).map(Token::Existing).collect()
            } else {
                splice_order::order(
                    &remaining_ids,
                    &restores,
                    catalog.matching(&key).chain(anchors.into_iter()),
                )?
            };
            if absence && !order.is_empty() {
                return Err(ScopedHistoryError::NodePathChanged);
            }
            let relocation = metadata::relocations(nodes, &removed, &order);
            parents.push(ParentPlan {
                scope: key.scope,
                path,
                groups,
                order,
                restore,
                remaining: remaining_ids.len(),
                absence,
                relocation,
            });
        }
        if check_collisions && !restoring_ids.is_empty() {
            let mut collision = false;
            identity::walk(&doc.children, |node, _| {
                collision |= restoring_ids.contains(node.id_str())
            });
            if let Some(pages) = &doc.pages {
                for page in pages {
                    identity::walk(&page.children, |node, _| {
                        collision |= restoring_ids.contains(node.id_str())
                    });
                }
            }
            if collision {
                return Err(ScopedHistoryError::NodePathChanged);
            }
        }
        parents.sort_by(|a, b| {
            let scope = match (&a.scope, &b.scope) {
                (Scope::Root, Scope::Root) => std::cmp::Ordering::Equal,
                (Scope::Root, _) => std::cmp::Ordering::Less,
                (_, Scope::Root) => std::cmp::Ordering::Greater,
                (Scope::Page { index: a, .. }, Scope::Page { index: b, .. }) => a.cmp(b),
            };
            scope.then_with(|| b.path.cmp(&a.path))
        });
        let metadata = metadata::routes(doc, patches, &index, &parents)?;
        Ok(Some(Self { parents, metadata }))
    }
    pub(super) fn rebase(&self, patches: &mut [Patch]) {
        for parent in &self.parents {
            for group in &parent.groups {
                patches[group.patch].splice_at_mut(group.splice).parent = parent.path.clone();
            }
        }
    }
    pub(super) fn exchange(self, doc: &mut PenDocument, patches: &mut [Patch]) {
        for parent in self.parents {
            let roots = patch::roots_mut(doc, &parent.scope).unwrap();
            let mut slot = None;
            let nodes = if parent.path.is_empty() {
                roots
            } else {
                let node = patch::node_mut(roots, &parent.path).unwrap();
                slot = patch::children_slot(node);
                slot.as_mut().unwrap().get_or_insert_with(Vec::new)
            };
            let remove: HashSet<_> = parent
                .groups
                .iter()
                .flat_map(|group| group.expected.iter().map(String::as_str))
                .collect();
            let mut removed = HashMap::new();
            for node in nodes.extract_if(.., |node| remove.contains(node.id_str())) {
                removed.insert(node.id_str().to_owned(), SharedNode::capture_owned(node));
            }
            debug_assert_eq!(nodes.len(), parent.remaining);
            nodes.reserve_exact(parent.restore.len());
            nodes.extend(parent.restore.iter().map(|node| node.materialize()));
            let mut destinations = vec![0; parent.order.len()];
            for (at, token) in parent.order.into_iter().enumerate() {
                let original = match token {
                    Token::Existing(at) => at,
                    Token::Restore(at) => parent.remaining + at,
                };
                destinations[original] = at;
            }
            for at in 0..destinations.len() {
                while destinations[at] != at {
                    let other = destinations[at];
                    nodes.swap(at, other);
                    destinations.swap(at, other);
                }
            }
            let total = nodes.len();
            let positions: HashMap<_, _> = nodes
                .iter()
                .enumerate()
                .map(|(at, node)| (node.id_str(), at))
                .collect();
            for group in parent.groups {
                let splice = patches[group.patch].splice_at_mut(group.splice);
                splice.old = group
                    .expected
                    .iter()
                    .map(|id| removed.remove(id).unwrap())
                    .collect();
                splice.expected_ids = group.restore;
                splice.expected_total = total;
                splice.parent = parent.path.clone();
                splice.expected_present = !parent.absence;
                splice.old_present = group.present;
                splice.recent = group.recent;
                splice.index = splice
                    .expected_ids
                    .first()
                    .and_then(|id| positions.get(id.as_str()).copied())
                    .unwrap_or(0);
            }
            if parent.absence {
                *slot.unwrap() = None;
            }
        }
        for (at, path) in self.metadata {
            let Patch::Node(node) = &mut patches[at] else {
                unreachable!("prepared metadata patch");
            };
            node.path = path;
        }
    }
}

impl Prepared {
    /// Project the exact inverse plan onto identities only. No canonical node
    /// is cloned/exchanged, and all sealed clone/paste anchor decisions come
    /// from the same planner used by rollback/undo.
    pub(super) fn forward_children<'a>(
        &self,
        doc: &'a PenDocument,
        budget: &mut super::forward_structure::Budget,
    ) -> Option<Vec<super::forward_structure::Children<'a>>> {
        use super::forward_structure::{ids, Children};
        let mut result = Vec::new();
        for parent in &self.parents {
            let roots = patch::roots(doc, &parent.scope).ok()?;
            let nodes = if parent.path.is_empty() {
                roots
            } else {
                patch::node(roots, &parent.path)?
                    .children()
                    .map_or(&[][..], Vec::as_slice)
            };
            if nodes.len() > 4096 || parent.order.len() > 4096 {
                return None;
            }
            let removed = parent
                .groups
                .iter()
                .flat_map(|group| group.expected.iter().map(String::as_str))
                .collect::<HashSet<_>>();
            let remaining = nodes
                .iter()
                .filter(|node| !removed.contains(node.id_str()))
                .collect::<Vec<_>>();
            let before = parent
                .order
                .iter()
                .map(|token| match token {
                    Token::Existing(at) => remaining.get(*at).map(|node| node.id_str()),
                    Token::Restore(at) => parent.restore.get(*at).map(|node| node.id_str()),
                })
                .collect::<Option<Vec<_>>>()?;
            result.push(Children::capture(
                doc,
                &parent.scope,
                &parent.path,
                ids(before)?,
                !parent.absence,
                budget,
            )?);
        }
        Some(result)
    }
}
