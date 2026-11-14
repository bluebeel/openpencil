//! Resolve only requested current identities, rejecting actual ambiguity.
use super::{
    patch::{self, Scope},
    ScopedHistoryError,
};
use crate::PenNodeExt;
use jian_ops_schema::{node::PenNode, PenDocument};
use std::collections::{HashMap, HashSet};
pub(super) fn walk<'a>(roots: &'a [PenNode], mut visit: impl FnMut(&'a PenNode, &[usize])) {
    let mut stack = vec![(roots, 0)];
    let mut path = Vec::new();
    while let Some((nodes, next)) = stack.last_mut() {
        if *next == nodes.len() {
            stack.pop();
            if !stack.is_empty() {
                path.pop();
            }
            continue;
        }
        let current = *nodes;
        let index = *next;
        *next += 1;
        let node = &current[index];
        path.push(index);
        visit(node, &path);
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            stack.push((children, 0));
        } else {
            path.pop();
        }
    }
}
pub(super) fn collect_ids(node: &PenNode, out: &mut Vec<String>) {
    out.push(node.id_str().to_owned());
    walk(node.children().map_or(&[][..], Vec::as_slice), |node, _| {
        out.push(node.id_str().to_owned())
    });
}
pub(super) type Wanted = HashMap<Scope, HashSet<String>>;
pub(super) struct Index(HashMap<Scope, HashMap<String, Option<Box<[usize]>>>>);
impl Index {
    pub(super) fn new(doc: &PenDocument, wanted: Wanted) -> Result<Self, ScopedHistoryError> {
        let mut scopes = HashMap::new();
        for (scope, wanted) in wanted {
            let roots = patch::roots(doc, &scope)?;
            let mut found = HashMap::new();
            walk(roots, |node, path| {
                if wanted.contains(node.id_str()) {
                    match found.entry(node.id_str().to_owned()) {
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(Some(path.into()));
                        }
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            entry.insert(None);
                        }
                    }
                }
            });
            scopes.insert(scope, found);
        }
        Ok(Self(scopes))
    }
    pub(super) fn contains(&self, scope: &Scope, id: &str) -> bool {
        self.0
            .get(scope)
            .is_some_and(|nodes| nodes.contains_key(id))
    }
    pub(super) fn path(&self, scope: &Scope, id: &str) -> Result<Box<[usize]>, ScopedHistoryError> {
        self.0
            .get(scope)
            .and_then(|nodes| nodes.get(id))
            .and_then(Option::as_ref)
            .cloned()
            .ok_or(ScopedHistoryError::NodePathChanged)
    }
}

pub(super) fn shared_has_id(root: &crate::SharedNode, id: &str) -> bool {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if node.id_str() == id {
            return true;
        }
        if let Some(children) = node.children() {
            pending.extend(children.iter().map(std::sync::Arc::as_ref));
        }
    }
    false
}

pub(super) fn changed_subtree_ids(
    old: &crate::SharedNode,
    current: &PenNode,
) -> std::sync::Arc<Vec<String>> {
    let mut ids = Vec::new();
    collect_ids(current, &mut ids);
    let mut pending = vec![old];
    while let Some(node) = pending.pop() {
        ids.push(node.id_str().to_owned());
        if let Some(children) = node.children() {
            pending.extend(children.iter().map(std::sync::Arc::as_ref));
        }
    }
    ids.sort_unstable();
    ids.dedup();
    std::sync::Arc::new(ids)
}
