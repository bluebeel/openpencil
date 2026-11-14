//! Exact changed-node swaps, retaining one historical side only.
use crate::{history_snapshot::SharedNode, pen_node_ext::clone_without_children, PenNodeExt};
use jian_ops_schema::{node::PenNode, page::PenPage, PenDocument};
use std::{mem, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scope {
    Root,
    Page { index: usize, id: String },
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct NodePatch {
    pub scope: Scope,
    pub path: Box<[usize]>,
    pub expected_id: String,
    pub root_id: String,
    pub metadata_only: bool,
    pub old: Arc<SharedNode>,
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ForestPatch {
    pub scope: Scope,
    pub expected_ids: Vec<String>,
    pub old: Vec<Arc<SharedNode>>,
}
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Patch {
    Node(NodePatch),
    Forest(ForestPatch),
    Order(OrderPatch),
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct OrderPatch {
    scope: Scope,
    old_order: Vec<String>,
    expected_ids: Vec<String>,
    old: Vec<Arc<SharedNode>>,
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
fn roots_mut<'a>(
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
fn node<'a>(nodes: &'a [PenNode], path: &[usize]) -> Option<&'a PenNode> {
    let (first, rest) = path.split_first()?;
    let mut found = nodes.get(*first)?;
    for index in rest {
        found = found.children()?.get(*index)?;
    }
    Some(found)
}
fn node_mut<'a>(nodes: &'a mut [PenNode], path: &[usize]) -> Option<&'a mut PenNode> {
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
fn children_slot(node: &mut PenNode) -> Option<&mut Option<Vec<PenNode>>> {
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
fn same_children(a: &PenNode, b: &PenNode) -> bool {
    match (a.children(), b.children()) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.id_str() == b.id_str())
        }
        _ => false,
    }
}
pub(super) fn compact_forest(
    old: Vec<PenNode>,
    current: &[PenNode],
    scope: Scope,
    out: &mut Vec<Patch>,
) {
    if old.len() != current.len()
        || old
            .iter()
            .zip(current)
            .any(|(a, b)| a.id_str() != b.id_str())
    {
        let current_by_id: std::collections::HashMap<_, _> = current
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
                    compact_node(old, node, &scope, node.id_str(), &mut vec![*index], out);
                } else {
                    removed.push(SharedNode::capture(&old, None));
                }
            }
            out.push(Patch::Order(OrderPatch {
                scope,
                old_order,
                expected_ids,
                old: removed,
            }));
            return;
        }
        out.push(Patch::Forest(ForestPatch {
            scope,
            expected_ids: current.iter().map(|n| n.id_str().to_owned()).collect(),
            old: old.iter().map(|n| SharedNode::capture(n, None)).collect(),
        }));
        return;
    }
    for (index, (old, current)) in old.into_iter().zip(current).enumerate() {
        compact_node(
            old,
            current,
            &scope,
            current.id_str(),
            &mut vec![index],
            out,
        );
    }
}
fn compact_node(
    mut old: PenNode,
    current: &PenNode,
    scope: &Scope,
    root_id: &str,
    path: &mut Vec<usize>,
    out: &mut Vec<Patch>,
) {
    if mem::discriminant(&old) != mem::discriminant(current) || !same_children(&old, current) {
        out.push(Patch::Node(NodePatch {
            scope: scope.clone(),
            path: path.clone().into_boxed_slice(),
            expected_id: current.id_str().to_owned(),
            root_id: root_id.to_owned(),
            metadata_only: false,
            old: SharedNode::capture(&old, None),
        }));
        return;
    }
    let old_children = children_slot(&mut old).and_then(Option::take);
    // Preserve the source's absent versus present-empty metadata shape.
    if old_children.is_some() {
        *children_slot(&mut old).unwrap() = Some(Vec::new());
    }
    let current_metadata = clone_without_children(current);
    if !crate::history_snapshot::metadata_matches(&old, &current_metadata) {
        out.push(Patch::Node(NodePatch {
            scope: scope.clone(),
            path: path.clone().into_boxed_slice(),
            expected_id: current.id_str().to_owned(),
            root_id: root_id.to_owned(),
            metadata_only: true,
            old: SharedNode::capture(&old, None),
        }));
    }
    if let Some(children) = old_children {
        for (index, (child, current)) in children
            .into_iter()
            .zip(current.children().unwrap())
            .enumerate()
        {
            path.push(index);
            compact_node(child, current, scope, root_id, path, out);
            path.pop();
        }
    }
}
impl Patch {
    pub(super) fn retained_roots(&self) -> &[Arc<SharedNode>] {
        match self {
            Self::Node(p) => std::slice::from_ref(&p.old),
            Self::Forest(p) => &p.old,
            Self::Order(p) => &p.old,
        }
    }
    pub(super) fn storage_bytes(&self) -> usize {
        let scope_bytes = |s: &Scope| match s {
            Scope::Root => 0,
            Scope::Page { id, .. } => id.capacity(),
        };
        match self {
            Self::Node(p) => {
                scope_bytes(&p.scope)
                    + p.path.len() * mem::size_of::<usize>()
                    + p.expected_id.capacity()
                    + p.root_id.capacity()
            }
            Self::Forest(p) => {
                scope_bytes(&p.scope)
                    + p.expected_ids.capacity() * mem::size_of::<String>()
                    + p.expected_ids.iter().map(String::capacity).sum::<usize>()
                    + p.old.capacity() * mem::size_of::<Arc<SharedNode>>()
            }
            Self::Order(p) => {
                scope_bytes(&p.scope)
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
    pub(super) fn order_scope(&self) -> Option<&Scope> {
        if let Self::Order(p) = self {
            Some(&p.scope)
        } else {
            None
        }
    }
    pub(super) fn rebase(&mut self, doc: &PenDocument, reordered: &[Scope]) {
        if let Self::Node(p) = self {
            if !reordered.contains(&p.scope) {
                return;
            }
            p.path[0] = roots(doc, &p.scope)
                .unwrap()
                .iter()
                .position(|node| node.id_str() == p.root_id)
                .expect("common root preserved by order swap");
        }
    }
    pub(super) fn validate(&self, doc: &PenDocument) -> Result<(), ScopedHistoryError> {
        match self {
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
                let found = roots(doc, &p.scope)?;
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
                let found = roots(doc, &p.scope)?;
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
    pub(super) fn exchange(&mut self, doc: &mut PenDocument) {
        match self {
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
                let found = roots_mut(doc, &p.scope).unwrap();
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
                let found = roots_mut(doc, &p.scope).unwrap();
                let reverse: Vec<_> = found.iter().map(|n| SharedNode::capture(n, None)).collect();
                *found = p.old.iter().map(|n| n.materialize()).collect();
                p.expected_ids = found.iter().map(|n| n.id_str().to_owned()).collect();
                p.old = reverse;
            }
        }
    }
}
