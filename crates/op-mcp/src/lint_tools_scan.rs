//! Preserve root context, issue order and last-postorder duplicate filter IDs.
use super::{render, Prepared};
use jian_ops_schema::node::PenNode;
use op_editor_core::{EditorState, PenNodeExt};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};
pub(super) fn roots(state: &EditorState) -> impl Iterator<Item = &PenNode> {
    state
        .doc
        .pages
        .iter()
        .flatten()
        .flat_map(|page| page.children.iter())
        .chain(state.doc.children.iter())
}
#[derive(Default)]
pub(super) struct Subtrees {
    ids: Vec<String>,
    ranges: BTreeMap<String, Range<usize>>,
}
impl Subtrees {
    pub(super) fn push(&mut self, root: &PenNode) {
        let mut frames = vec![(root, 0usize, self.ids.len())];
        self.ids.push(root.id_str().to_owned());
        while let Some((node, next, start)) = frames.last_mut() {
            let current = *node;
            if let Some(child) = current.children().and_then(|children| children.get(*next)) {
                *next += 1;
                let start = self.ids.len();
                self.ids.push(child.id_str().to_owned());
                frames.push((child, 0, start));
            } else {
                self.ranges
                    .insert(current.id_str().to_owned(), *start..self.ids.len());
                frames.pop();
            }
        }
    }
    pub(super) fn allowed(&self, id: &str) -> Option<BTreeSet<&str>> {
        Some(
            self.ids
                .get(self.ranges.get(id)?.clone())?
                .iter()
                .map(String::as_str)
                .collect(),
        )
    }
}
fn visit_postorder<'a>(root: &'a PenNode, mut visit: impl FnMut(&'a PenNode)) {
    let mut frames = vec![(root, 0usize)];
    while let Some((node, next)) = frames.last_mut() {
        let current = *node;
        if let Some(child) = current.children().and_then(|children| children.get(*next)) {
            *next += 1;
            frames.push((child, 0));
        } else {
            visit(current);
            frames.pop();
        }
    }
}
fn allowed<'a>(state: &'a EditorState, filter: &str) -> Option<BTreeSet<&'a str>> {
    let mut selected = None;
    for root in roots(state) {
        visit_postorder(root, |node| {
            if node.id_str() == filter {
                selected = Some(node);
            }
        });
    }
    let mut ids = BTreeSet::new();
    visit_postorder(selected?, |node| {
        ids.insert(node.id_str());
    });
    Some(ids)
}
fn intersects(root: &PenNode, wanted: &BTreeSet<&str>) -> bool {
    let mut pending = vec![std::slice::from_ref(root).iter()];
    while let Some(nodes) = pending.last_mut() {
        let Some(node) = nodes.next() else {
            pending.pop();
            continue;
        };
        if wanted.contains(node.id_str()) {
            return true;
        }
        if let Some(children) = node.children() {
            pending.push(children.iter());
        }
    }
    false
}
pub(super) fn prepare(state: &EditorState, filter: Option<&str>) -> Prepared {
    let allowed = match filter {
        Some(id) => Some(allowed(state, id).ok_or_else(|| format!("nodeId not found: {id}"))?),
        None => None,
    };
    let mut issues = Vec::new();
    for root in roots(state) {
        // Every detector emits findings on nodes within its supplied root.
        // Keep complete root/ancestor/sibling context and detect_all dedup; only
        // roots with no possible accepted issue ID are omitted. Duplicate IDs
        // in other roots remain candidates, exactly as the original ID filter.
        if allowed.as_ref().is_none_or(|ids| intersects(root, ids)) {
            issues.extend(op_design_lint::detect_all(root, &state.doc));
        }
    }
    Ok(render(&issues, allowed.as_ref()))
}
