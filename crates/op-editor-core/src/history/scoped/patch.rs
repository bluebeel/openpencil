//! Exact changed-node swaps, retaining one historical side only.
use crate::{history_snapshot::SharedNode, pen_node_ext::clone_without_children, PenNodeExt};
use jian_ops_schema::{node::PenNode, page::PenPage, PenDocument};
use std::{collections::HashMap, mem, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Scope {
    Root,
    Page { index: usize, id: String },
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct NodePatch {
    pub scope: Scope,
    pub path: Box<[usize]>,
    pub expected_id: String,
    pub metadata_only: bool,
    pub old: Arc<SharedNode>,
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ForestPatch {
    pub scope: Scope,
    parent: Box<[usize]>,
    parent_id: Option<String>,
    pub expected_ids: Vec<String>,
    pub old: Vec<Arc<SharedNode>>,
}
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Patch {
    Node(NodePatch),
    Forest(ForestPatch),
    Order(OrderPatch),
    Splice(super::splice::SplicePatch),
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct OrderPatch {
    scope: Scope,
    parent: Box<[usize]>,
    parent_id: Option<String>,
    old_order: Vec<String>,
    expected_ids: Vec<String>,
    old: Vec<Arc<SharedNode>>,
}
/// Every mapping uses the pre-exchange structural coordinates. Descendant
/// swaps execute before ancestor permutations; paths are rebased only once all
/// swaps finish. Borrowed prefix lookup avoids scanning every order per patch.
#[derive(Default)]
pub(super) struct Reorders(HashMap<Scope, HashMap<Box<[usize]>, Vec<Option<usize>>>>);
impl Reorders {
    fn record(&mut self, patch: &OrderPatch) {
        let destinations: HashMap<_, _> = patch
            .old_order
            .iter()
            .enumerate()
            .map(|(index, id)| (id.as_str(), index))
            .collect();
        let indices = patch
            .expected_ids
            .iter()
            .map(|id| destinations.get(id.as_str()).copied())
            .collect();
        self.0
            .entry(patch.scope.clone())
            .or_default()
            .insert(patch.parent.clone(), indices);
    }
    fn rebase(&self, scope: &Scope, path: &mut [usize]) {
        let Some(orders) = self.0.get(scope) else {
            return;
        };
        let original = path.to_vec();
        for depth in 0..original.len() {
            if let Some(mapping) = orders.get(&original[..depth]) {
                path[depth] =
                    mapping[original[depth]].expect("retained patch belongs to a common child");
            }
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct PageHeader {
    pub index: usize,
    pub old: PenPage,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopedHistoryError {
    DocumentGenerationChanged { expected: u64, actual: u64 },
    PageShapeChanged,
    PageMissing { index: usize },
    PageIdentityChanged { index: usize },
    NodePathChanged,
    DocumentHeaderChanged,
}
impl std::fmt::Display for ScopedHistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DocumentGenerationChanged { expected, actual } => write!(
                f,
                "document generation changed: expected {expected}, got {actual}"
            ),
            Self::PageShapeChanged => f.write_str("page shape changed during scoped edit"),
            Self::PageMissing { index } => write!(f, "page {index} is missing during scoped edit"),
            Self::PageIdentityChanged { index } => {
                write!(f, "page {index} changed identity during scoped edit")
            }
            Self::DocumentHeaderChanged => {
                f.write_str("document header shape changed during scoped edit")
            }
            Self::NodePathChanged => f.write_str("node path changed during scoped edit"),
        }
    }
}
impl std::error::Error for ScopedHistoryError {}
pub(super) fn roots<'a>(
    doc: &'a PenDocument,
    scope: &Scope,
) -> Result<&'a [PenNode], ScopedHistoryError> {
    match scope {
        Scope::Root => Ok(&doc.children),
        Scope::Page { index, id } => {
            let page = doc
                .pages
                .as_ref()
                .and_then(|p| p.get(*index))
                .ok_or(ScopedHistoryError::PageMissing { index: *index })?;
            if &page.id != id {
                return Err(ScopedHistoryError::PageIdentityChanged { index: *index });
            }
            Ok(&page.children)
        }
    }
}
pub(super) fn roots_mut<'a>(
    doc: &'a mut PenDocument,
    scope: &Scope,
) -> Result<&'a mut Vec<PenNode>, ScopedHistoryError> {
    match scope {
        Scope::Root => Ok(&mut doc.children),
        Scope::Page { index, id } => {
            let page = doc
                .pages
                .as_mut()
                .and_then(|p| p.get_mut(*index))
                .ok_or(ScopedHistoryError::PageMissing { index: *index })?;
            if &page.id != id {
                return Err(ScopedHistoryError::PageIdentityChanged { index: *index });
            }
            Ok(&mut page.children)
        }
    }
}
pub(super) fn node<'a>(nodes: &'a [PenNode], path: &[usize]) -> Option<&'a PenNode> {
    let (first, rest) = path.split_first()?;
    let mut found = nodes.get(*first)?;
    for index in rest {
        found = found.children()?.get(*index)?;
    }
    Some(found)
}
pub(super) fn node_mut<'a>(nodes: &'a mut [PenNode], path: &[usize]) -> Option<&'a mut PenNode> {
    let (first, rest) = path.split_first()?;
    let mut found = nodes.get_mut(*first)?;
    for index in rest {
        if found.children().is_none() {
            return None;
        }
        found = found.children_mut()?.get_mut(*index)?;
    }
    Some(found)
}
fn child_list<'a>(
    doc: &'a PenDocument,
    scope: &Scope,
    parent: &[usize],
    parent_id: Option<&str>,
) -> Result<&'a [PenNode], ScopedHistoryError> {
    let roots = roots(doc, scope)?;
    if parent.is_empty() {
        return Ok(roots);
    }
    let parent = node(roots, parent).ok_or(ScopedHistoryError::NodePathChanged)?;
    if Some(parent.id_str()) != parent_id {
        return Err(ScopedHistoryError::NodePathChanged);
    }
    parent
        .children()
        .map(Vec::as_slice)
        .ok_or(ScopedHistoryError::NodePathChanged)
}
fn child_list_mut<'a>(
    doc: &'a mut PenDocument,
    scope: &Scope,
    parent: &[usize],
) -> &'a mut Vec<PenNode> {
    let roots = roots_mut(doc, scope).unwrap();
    if parent.is_empty() {
        return roots;
    }
    // Every destination was validated before any swap; this does not author
    // absent children or search through a compatibility mutator.
    node_mut(roots, parent).unwrap().children_mut().unwrap()
}
pub(super) fn children_slot(node: &mut PenNode) -> Option<&mut Option<Vec<PenNode>>> {
    match node {
        PenNode::Frame(n) => Some(&mut n.children),
        PenNode::Group(n) => Some(&mut n.children),
        PenNode::Rectangle(n) => Some(&mut n.children),
        PenNode::Tabs(n) => Some(&mut n.children),
        PenNode::Ref(n) => Some(&mut n.children),
        PenNode::Ellipse(_)
        | PenNode::Line(_)
        | PenNode::Polygon(_)
        | PenNode::Path(_)
        | PenNode::Text(_)
        | PenNode::TextInput(_)
        | PenNode::Image(_)
        | PenNode::IconFont(_)
        | PenNode::TextArea(_)
        | PenNode::Select(_)
        | PenNode::Switch(_)
        | PenNode::Checkbox(_)
        | PenNode::Slider(_)
        | PenNode::RadioGroup(_)
        | PenNode::NumberInput(_)
        | PenNode::Progress(_) => None,
    }
}
pub(super) fn compact_forest(
    old: Vec<PenNode>,
    current: &[PenNode],
    scope: Scope,
    out: &mut Vec<Patch>,
) {
    compact_list(old, current, &scope, &mut Vec::new(), None, out);
}
fn compact_list(
    old: Vec<PenNode>,
    current: &[PenNode],
    scope: &Scope,
    parent: &mut Vec<usize>,
    parent_id: Option<&str>,
    out: &mut Vec<Patch>,
) {
    if old.len() != current.len()
        || old
            .iter()
            .zip(current)
            .any(|(a, b)| a.id_str() != b.id_str())
    {
        let current_by_id: HashMap<_, _> = current
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id_str(), (index, node)))
            .collect();
        let old_unique = old
            .iter()
            .map(PenNodeExt::id_str)
            .collect::<std::collections::HashSet<_>>()
            .len();
        if current_by_id.len() == current.len() && old_unique == old.len() {
            let old_order = old.iter().map(|node| node.id_str().to_owned()).collect();
            let expected_ids = current
                .iter()
                .map(|node| node.id_str().to_owned())
                .collect();
            let mut removed = Vec::new();
            for old in old {
                if let Some((index, node)) = current_by_id.get(old.id_str()) {
                    parent.push(*index);
                    compact_node(old, node, scope, parent, out);
                    parent.pop();
                } else {
                    removed.push(SharedNode::capture(&old, None));
                }
            }
            out.push(Patch::Order(OrderPatch {
                scope: scope.clone(),
                parent: parent.clone().into_boxed_slice(),
                parent_id: parent_id.map(str::to_owned),
                old_order,
                expected_ids,
                old: removed,
            }));
            return;
        }
        // Duplicate siblings cannot be identified by ID alone. Preserve the
        // exact child forest at this one parent; unrelated ancestors stay out.
        out.push(Patch::Forest(ForestPatch {
            scope: scope.clone(),
            parent: parent.clone().into_boxed_slice(),
            parent_id: parent_id.map(str::to_owned),
            expected_ids: current
                .iter()
                .map(|node| node.id_str().to_owned())
                .collect(),
            old: old
                .iter()
                .map(|node| SharedNode::capture(node, None))
                .collect(),
        }));
        return;
    }
    for (index, (old, current)) in old.into_iter().zip(current).enumerate() {
        parent.push(index);
        compact_node(old, current, scope, parent, out);
        parent.pop();
    }
}
fn compact_node(
    mut old: PenNode,
    current: &PenNode,
    scope: &Scope,
    path: &mut Vec<usize>,
    out: &mut Vec<Patch>,
) {
    if mem::discriminant(&old) != mem::discriminant(current)
        || old.children().is_some() != current.children().is_some()
    {
        out.push(Patch::Node(NodePatch {
            scope: scope.clone(),
            path: path.clone().into_boxed_slice(),
            expected_id: current.id_str().to_owned(),
            metadata_only: false,
            old: SharedNode::capture(&old, None),
        }));
        return;
    }
    let old_children = children_slot(&mut old).and_then(Option::take);
    if old_children.is_some() {
        *children_slot(&mut old).unwrap() = Some(Vec::new());
    }
    let current_metadata = clone_without_children(current);
    if !crate::history_snapshot::metadata_matches(&old, &current_metadata) {
        out.push(Patch::Node(NodePatch {
            scope: scope.clone(),
            path: path.clone().into_boxed_slice(),
            expected_id: current.id_str().to_owned(),
            metadata_only: true,
            old: SharedNode::capture(&old, None),
        }));
    }
    if let Some(children) = old_children {
        compact_list(
            children,
            current.children().unwrap(),
            scope,
            path,
            Some(current.id_str()),
            out,
        );
    }
}
impl Patch {
    pub(super) fn retained_roots(&self) -> &[Arc<SharedNode>] {
        match self {
            Self::Node(p) => std::slice::from_ref(&p.old),
            Self::Forest(p) => &p.old,
            Self::Order(p) => &p.old,
            Self::Splice(p) => &p.old,
        }
    }
    pub(super) fn storage_bytes(&self) -> usize {
        let scope_bytes = |s: &Scope| match s {
            Scope::Root => 0,
            Scope::Page { id, .. } => id.capacity(),
        };
        match self {
            Self::Splice(p) => p.storage_bytes(),
            Self::Node(p) => {
                scope_bytes(&p.scope)
                    + p.path.len() * mem::size_of::<usize>()
                    + p.expected_id.capacity()
            }
            Self::Forest(p) => {
                scope_bytes(&p.scope)
                    + p.parent.len() * mem::size_of::<usize>()
                    + p.parent_id.as_ref().map_or(0, String::capacity)
                    + p.expected_ids.capacity() * mem::size_of::<String>()
                    + p.expected_ids.iter().map(String::capacity).sum::<usize>()
                    + p.old.capacity() * mem::size_of::<Arc<SharedNode>>()
            }
            Self::Order(p) => {
                scope_bytes(&p.scope)
                    + p.parent.len() * mem::size_of::<usize>()
                    + p.parent_id.as_ref().map_or(0, String::capacity)
                    + (p.expected_ids.capacity() + p.old_order.capacity())
                        * mem::size_of::<String>()
                    + p.expected_ids
                        .iter()
                        .chain(&p.old_order)
                        .map(String::capacity)
                        .sum::<usize>()
                    + p.old.capacity() * mem::size_of::<Arc<SharedNode>>()
            }
        }
    }
    pub(super) fn rebase(&mut self, reordered: &Reorders) {
        match self {
            Self::Node(p) => reordered.rebase(&p.scope, &mut p.path),
            Self::Forest(p) => reordered.rebase(&p.scope, &mut p.parent),
            Self::Order(p) => reordered.rebase(&p.scope, &mut p.parent),
            Self::Splice(p) => reordered.rebase(&p.scope, &mut p.parent),
        }
    }
    pub(super) fn validate(&self, doc: &PenDocument) -> Result<(), ScopedHistoryError> {
        match self {
            Self::Splice(p) => p.validate(doc)?,
            Self::Node(p) => {
                let found = node(roots(doc, &p.scope)?, &p.path)
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
                if found.id_str() != p.expected_id {
                    return Err(ScopedHistoryError::NodePathChanged);
                }
                if p.metadata_only
                    && (mem::discriminant(found) != mem::discriminant(p.old.metadata())
                        || found.children().is_some() != p.old.children().is_some())
                {
                    return Err(ScopedHistoryError::NodePathChanged);
                }
            }
            Self::Order(p) => {
                let found = child_list(doc, &p.scope, &p.parent, p.parent_id.as_deref())?;
                if found.len() != p.expected_ids.len()
                    || found
                        .iter()
                        .zip(&p.expected_ids)
                        .any(|(node, id)| node.id_str() != id)
                {
                    return Err(ScopedHistoryError::NodePathChanged);
                }
            }
            Self::Forest(p) => {
                let found = child_list(doc, &p.scope, &p.parent, p.parent_id.as_deref())?;
                if found.len() != p.expected_ids.len()
                    || found
                        .iter()
                        .zip(&p.expected_ids)
                        .any(|(n, id)| n.id_str() != id)
                {
                    return Err(ScopedHistoryError::NodePathChanged);
                }
            }
        }
        Ok(())
    }
    /// Called only after all patches have validated, while the authority is locked.
    pub(super) fn exchange(&mut self, doc: &mut PenDocument, reordered: &mut Reorders) {
        match self {
            Self::Splice(p) => p.exchange(doc),
            Self::Node(p) => {
                let found = node_mut(roots_mut(doc, &p.scope).unwrap(), &p.path).unwrap();
                let reverse = if p.metadata_only {
                    SharedNode::capture(&clone_without_children(found), Some(&p.old))
                } else {
                    SharedNode::capture(found, Some(&p.old))
                };
                let children = if p.metadata_only {
                    children_slot(found).and_then(Option::take)
                } else {
                    None
                };
                *found = p.old.materialize();
                if p.metadata_only {
                    if let Some(slot) = children_slot(found) {
                        *slot = children;
                    }
                }
                p.expected_id = found.id_str().to_owned();
                p.old = reverse;
            }
            Self::Order(p) => {
                reordered.record(p);
                let found = child_list_mut(doc, &p.scope, &p.parent);
                let wanted: std::collections::HashSet<_> =
                    p.old_order.iter().map(String::as_str).collect();
                let reverse = found
                    .iter()
                    .filter(|node| !wanted.contains(node.id_str()))
                    .map(|node| SharedNode::capture(node, None))
                    .collect();
                found.retain(|node| wanted.contains(node.id_str()));
                found.reserve_exact(p.old_order.len().saturating_sub(found.len()));
                found.extend(p.old.iter().map(|node| node.materialize()));
                let mut positions: std::collections::HashMap<_, _> = found
                    .iter()
                    .enumerate()
                    .map(|(index, node)| (node.id_str().to_owned(), index))
                    .collect();
                for (index, id) in p.old_order.iter().enumerate() {
                    let old_index = positions[id];
                    if old_index != index {
                        found.swap(index, old_index);
                        *positions.get_mut(found[index].id_str()).unwrap() = index;
                        *positions.get_mut(found[old_index].id_str()).unwrap() = old_index;
                    }
                }
                drop(wanted);
                mem::swap(&mut p.old_order, &mut p.expected_ids);
                p.old = reverse;
            }
            Self::Forest(p) => {
                let found = child_list_mut(doc, &p.scope, &p.parent);
                let reverse: Vec<_> = found.iter().map(|n| SharedNode::capture(n, None)).collect();
                *found = p.old.iter().map(|n| n.materialize()).collect();
                p.expected_ids = found.iter().map(|n| n.id_str().to_owned()).collect();
                p.old = reverse;
            }
        }
    }
}
