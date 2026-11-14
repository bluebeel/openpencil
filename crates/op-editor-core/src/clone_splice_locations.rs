//! Compact index receipts come from the actual prepared mutation, not from
//! predicting clipboard/selection semantics a second time.
use super::{plan::Plan, Prepared};
use crate::EditorState;
#[derive(Debug)]
pub(crate) struct LocationGap {
    pub parent: Box<[usize]>,
    pub at: usize,
    pub count: usize,
}
impl Plan {
    fn locations(&self) -> Vec<LocationGap> {
        let mut gaps = Vec::new();
        for list in &self.lists {
            for (at, chain) in &list.gaps {
                gaps.push(LocationGap {
                    parent: list.path.clone(),
                    at: *at,
                    count: chain.len,
                });
            }
        }
        gaps
    }
}
impl Prepared {
    pub(crate) fn commit_with_locations(self, state: &mut EditorState) -> Vec<crate::NodeId> {
        let receipt = state.prepare_clone_locations(|| self.plan.locations());
        let ids = self.commit(state.active_children_mut());
        if let Some(receipt) = receipt {
            receipt.finish(state, !ids.is_empty());
        }
        ids
    }
}
