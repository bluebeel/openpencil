//! Identity-checked structural routes. Traversal never creates absent children.
use super::{patch, ScopedHistoryError};
use crate::PenNodeExt;
use jian_ops_schema::{node::PenNode, PenDocument};
use std::mem;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Location {
    pub parent: Box<[usize]>,
    pub index: usize,
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Parent {
    pub path: Box<[usize]>,
    id: Option<String>,
    kind: Option<mem::Discriminant<PenNode>>,
}
pub(super) fn identity(node: Option<&PenNode>) -> Result<Option<String>, ScopedHistoryError> {
    node.map(|node| {
        (node.id_str().len() <= 16 * 1024)
            .then(|| node.id_str().to_owned())
            .ok_or(ScopedHistoryError::NodePathChanged)
    })
    .transpose()
}
impl Parent {
    pub(super) fn capture(
        doc: &PenDocument,
        scope: &patch::Scope,
        path: Box<[usize]>,
    ) -> Result<Self, ScopedHistoryError> {
        if path.len() > 128 {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let roots = patch::roots(doc, scope)?;
        let (id, kind) = if path.is_empty() {
            (None, None)
        } else {
            let node = patch::node(roots, &path).ok_or(ScopedHistoryError::NodePathChanged)?;
            if !node.is_container() || matches!(node, PenNode::Ref(_)) {
                return Err(ScopedHistoryError::NodePathChanged);
            }
            (identity(Some(node))?, Some(mem::discriminant(node)))
        };
        Ok(Self { path, id, kind })
    }
    pub(super) fn list<'a>(
        &self,
        doc: &'a PenDocument,
        scope: &patch::Scope,
    ) -> Result<(&'a [PenNode], bool), ScopedHistoryError> {
        let roots = patch::roots(doc, scope)?;
        if self.path.is_empty() {
            return Ok((roots, true));
        }
        let parent = patch::node(roots, &self.path).ok_or(ScopedHistoryError::NodePathChanged)?;
        if Some(parent.id_str()) != self.id.as_deref()
            || Some(mem::discriminant(parent)) != self.kind
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        Ok((
            parent.children().map_or(&[], Vec::as_slice),
            parent.children().is_some(),
        ))
    }
    pub(super) fn bytes(&self) -> usize {
        self.path.len() * mem::size_of::<usize>() + self.id.as_ref().map_or(0, String::capacity)
    }
}
pub(super) fn list_mut<'a>(
    doc: &'a mut PenDocument,
    scope: &patch::Scope,
    path: &[usize],
) -> &'a mut Vec<PenNode> {
    let roots = patch::roots_mut(doc, scope).unwrap();
    if path.is_empty() {
        return roots;
    }
    patch::children_slot(patch::node_mut(roots, path).unwrap())
        .unwrap()
        .get_or_insert_with(Vec::new)
}
pub(super) fn restore_absence(
    doc: &mut PenDocument,
    scope: &patch::Scope,
    path: &[usize],
    present: bool,
) {
    if present || path.is_empty() {
        return;
    }
    let roots = patch::roots_mut(doc, scope).unwrap();
    let slot = patch::children_slot(patch::node_mut(roots, path).unwrap()).unwrap();
    debug_assert!(slot.as_ref().is_none_or(Vec::is_empty));
    *slot = None;
}
/// Update an unaffected parent route after removing/inserting one sibling.
pub(super) fn removed(
    path: &mut [usize],
    parent: &[usize],
    index: usize,
) -> Result<(), ScopedHistoryError> {
    if path.len() > parent.len() && path.starts_with(parent) {
        match path[parent.len()].cmp(&index) {
            std::cmp::Ordering::Less => {}
            std::cmp::Ordering::Equal => return Err(ScopedHistoryError::NodePathChanged),
            std::cmp::Ordering::Greater => path[parent.len()] -= 1,
        }
    }
    Ok(())
}
pub(super) fn inserted(path: &mut [usize], parent: &[usize], index: usize) {
    if path.len() > parent.len() && path.starts_with(parent) && path[parent.len()] >= index {
        path[parent.len()] += 1;
    }
}
pub(super) fn scope_bytes(scope: &patch::Scope) -> usize {
    match scope {
        patch::Scope::Root => 0,
        patch::Scope::Page { id, .. } => id.capacity(),
    }
}
