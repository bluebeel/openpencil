//! Owned insertion identities extracted from prepared payloads, not from a page clone.
use super::{lookup, plan::Plan, Prepared};
use crate::PenNodeExt;
use jian_ops_schema::node::PenNode;
use std::mem;
#[derive(Debug)]
pub(crate) struct Trace {
    pub parent: Box<[usize]>,
    pub parent_id: Option<String>,
    pub parent_kind: Option<mem::Discriminant<PenNode>>,
    pub present: bool,
    pub left: Option<String>,
    pub right: Option<String>,
    pub roots: Vec<String>,
    pub changed: Vec<String>,
}
impl Prepared {
    pub(crate) fn traces(&self, roots: &[PenNode]) -> Vec<Trace> {
        self.plan.traces(roots)
    }
}
impl Plan {
    pub(super) fn traces(&self, roots: &[PenNode]) -> Vec<Trace> {
        let mut traces = Vec::new();
        for list in &self.lists {
            let (nodes, parent_id, parent_kind, present) = if list.path.is_empty() {
                (roots, None, None, true)
            } else {
                let parent = lookup::at(roots, &list.path).unwrap();
                (
                    parent.children().map_or(&[][..], Vec::as_slice),
                    Some(parent.id_str().to_owned()),
                    Some(mem::discriminant(parent)),
                    parent.children().is_some(),
                )
            };
            for (gap, chain) in &list.gaps {
                let mut ids = Vec::with_capacity(chain.len);
                let mut changed = Vec::new();
                let mut next = Some(chain.first);
                while let Some(id) = next {
                    let node = self.pending[id].node.as_ref().unwrap();
                    ids.push(node.id_str().to_owned());
                    changed.push(node.id_str().to_owned());
                    lookup::walk(node.children().map_or(&[][..], Vec::as_slice), |node, _| {
                        changed.push(node.id_str().to_owned())
                    });
                    next = self.pending[id].next;
                }
                changed.sort_unstable();
                changed.dedup();
                traces.push(Trace {
                    parent: list.path.clone(),
                    parent_id: parent_id.clone(),
                    parent_kind,
                    present,
                    left: gap
                        .checked_sub(1)
                        .and_then(|index| nodes.get(index))
                        .map(|node| node.id_str().to_owned()),
                    right: nodes.get(*gap).map(|node| node.id_str().to_owned()),
                    roots: ids,
                    changed,
                });
            }
        }
        traces
    }
}
