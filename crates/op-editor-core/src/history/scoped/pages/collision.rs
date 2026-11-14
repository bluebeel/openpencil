//! Retain exact pre-existing external collision counts, including legacy
//! duplicate/empty child ids. Restoration may not introduce a new collision.
use crate::{EditorState, PenNodeExt};
use jian_ops_schema::{node::PenNode, page::PenPage};
use std::collections::{HashMap, HashSet};

fn nodes(nodes: &[PenNode], visit: &mut impl FnMut(&str)) {
    let mut pending = vec![nodes.iter()];
    while let Some(siblings) = pending.last_mut() {
        if let Some(node) = siblings.next() {
            visit(node.id_str());
            if let Some(children) = node.children() {
                pending.push(children.iter());
            }
        } else {
            pending.pop();
        }
    }
}
pub(super) fn page_ids(page: &PenPage, ids: &mut HashSet<String>) {
    ids.insert(page.id.clone());
    nodes(&page.children, &mut |id| {
        ids.insert(id.to_owned());
    });
}
pub(super) fn external(
    state: &EditorState,
    wanted: &HashSet<String>,
    excluding: &HashSet<String>,
) -> HashMap<String, usize> {
    if wanted.is_empty() {
        return HashMap::new();
    }
    let mut result = HashMap::new();
    let mut count = |id: &str| {
        if wanted.contains(id) {
            *result.entry(id.to_owned()).or_insert(0) += 1;
        }
    };
    for page in state
        .doc
        .pages
        .iter()
        .flatten()
        .filter(|page| !excluding.contains(&page.id))
    {
        count(&page.id);
        nodes(&page.children, &mut count);
    }
    nodes(&state.doc.children, &mut count);
    result
}
pub(super) fn for_removed(
    state: &EditorState,
    removing: &HashSet<String>,
) -> HashMap<String, usize> {
    let mut wanted = HashSet::new();
    for page in state
        .doc
        .pages
        .iter()
        .flatten()
        .filter(|page| removing.contains(&page.id))
    {
        page_ids(page, &mut wanted);
    }
    external(state, &wanted, removing)
}
pub(super) fn validate(
    state: &EditorState,
    patch: &super::Patch,
    removing: &HashSet<String>,
) -> bool {
    let super::Kind::Splice { old, .. } = &patch.kind else {
        return true;
    };
    let mut wanted = HashSet::new();
    for page in old {
        wanted.insert(page.id().to_owned());
        let mut pending = page
            .children
            .iter()
            .map(std::sync::Arc::as_ref)
            .collect::<Vec<_>>();
        while let Some(node) = pending.pop() {
            wanted.insert(node.id_str().to_owned());
            if let Some(children) = node.children() {
                pending.extend(children.iter().map(std::sync::Arc::as_ref));
            }
        }
    }
    let actual = external(state, &wanted, removing);
    wanted.iter().all(|id| {
        actual.get(id).copied().unwrap_or(0) == patch.external.get(id).copied().unwrap_or(0)
    })
}
