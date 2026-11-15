//! Exact post-state child forests with original identity-order preconditions.
//! Existing inverse planners supply BEFORE order; no document/command replay.

use super::{
    forward::Page,
    patch::{self, Scope},
};
use crate::{PenDocument, PenNodeExt};
use jian_ops_schema::node::PenNode;
use serde::{Serialize, Serializer};

const MAX_NODES: usize = 4096;
const MAX_ID_BYTES: usize = 256 * 1024;

pub(super) struct Budget(usize);
impl Budget {
    pub(super) fn new(limit: usize) -> Self {
        Self(limit.saturating_mul(4).min(1024 * 1024))
    }
    fn reserve(&mut self, bytes: usize) -> Option<()> {
        self.0 = self.0.checked_sub(bytes)?;
        Some(())
    }
}

pub(super) fn ids<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<Vec<String>> {
    let mut result = Vec::new();
    let mut bytes = 0usize;
    for id in values {
        bytes = bytes.checked_add(id.len())?;
        if result.len() == MAX_NODES || bytes > MAX_ID_BYTES {
            return None;
        }
        result.push(id.to_owned());
    }
    Some(result)
}
pub(super) fn node_ids(nodes: &[PenNode]) -> Option<Vec<String>> {
    ids(nodes.iter().map(PenNodeExt::id_str))
}
pub(super) fn bounded_tree(nodes: &[PenNode]) -> bool {
    if nodes.len() > MAX_NODES {
        return false;
    }
    let mut pending = nodes.iter().map(|node| (node, 0usize)).collect::<Vec<_>>();
    let mut visited = 0;
    while let Some((node, depth)) = pending.pop() {
        visited += 1;
        if visited > MAX_NODES || depth > 128 {
            return false;
        }
        if let Some(children) = node.children() {
            if children.len() > MAX_NODES.saturating_sub(visited + pending.len()) {
                return false;
            }
            pending.extend(children.iter().map(|node| (node, depth + 1)));
        }
    }
    true
}

pub(super) struct Children<'a> {
    pub scope: Scope,
    pub parent: Box<[usize]>,
    parent_ids: Vec<String>,
    pub expected_ids: Vec<String>,
    pub expected_present: bool,
    value: Option<&'a [PenNode]>,
}
impl<'a> Children<'a> {
    pub(super) fn capture(
        doc: &'a PenDocument,
        scope: &Scope,
        parent: &[usize],
        expected_ids: Vec<String>,
        expected_present: bool,
        budget: &mut Budget,
    ) -> Option<Self> {
        if parent.is_empty() && !expected_present {
            return None;
        }
        let mut nodes = patch::roots(doc, scope).ok()?;
        let mut parent_ids = Vec::new();
        let mut value = Some(nodes);
        for at in parent {
            let node = nodes.get(*at)?;
            if nodes
                .iter()
                .filter(|sibling| sibling.id_str() == node.id_str())
                .count()
                != 1
            {
                return None;
            }
            parent_ids.push(node.id_str().to_owned());
            value = node.children().map(Vec::as_slice);
            nodes = value.unwrap_or(&[]);
        }
        if parent.len() > 128 || !bounded_tree(value.unwrap_or(&[])) {
            return None;
        }
        // Unique sibling identities make both the before-order precondition
        // and the stable ancestry route unambiguous. Duplicates request resync.
        let unique = |values: &[String]| {
            values
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                == values.len()
        };
        if !unique(&expected_ids) {
            return None;
        }
        let after = node_ids(value.unwrap_or(&[]))?;
        if !unique(&after) || (!expected_present && !expected_ids.is_empty()) {
            return None;
        }
        budget.reserve(
            (expected_ids.len() + parent_ids.len()) * std::mem::size_of::<String>()
                + expected_ids
                    .iter()
                    .chain(&parent_ids)
                    .map(String::len)
                    .sum::<usize>()
                + parent.len() * std::mem::size_of::<usize>(),
        )?;
        Some(Self {
            scope: scope.clone(),
            parent: parent.into(),
            parent_ids,
            expected_ids,
            expected_present,
            value,
        })
    }
    pub(super) fn covers(&self, scope: &Scope, path: &[usize]) -> bool {
        self.scope == *scope && path.len() > self.parent.len() && path.starts_with(&self.parent)
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Wire<'a> {
    op: &'static str,
    page: Option<Page<'a>>,
    parent_ids: &'a [String],
    expected_ids: &'a [String],
    expected_present: bool,
    value: Option<&'a [PenNode]>,
}
impl Serialize for Children<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Wire {
            op: "replace-children",
            page: Page::for_scope(&self.scope),
            parent_ids: &self.parent_ids,
            expected_ids: &self.expected_ids,
            expected_present: self.expected_present,
            value: self.value,
        }
        .serialize(serializer)
    }
}
