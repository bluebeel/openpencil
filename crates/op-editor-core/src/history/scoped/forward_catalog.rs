//! Coalesce repeated label updates into one exact external undo inverse.
use super::{auxiliary, ScopedHistoryError};
use crate::{
    web_sync::{committed::CatalogLabel, CommittedFeedError as Error},
    EditorState, NodeId,
};
use std::collections::HashMap;
struct Old {
    index: usize,
    id: NodeId,
    name: String,
}
#[derive(Default)]
pub(super) struct Labels {
    old: Vec<Old>,
    by_id: HashMap<NodeId, usize>,
}
impl Labels {
    pub(super) fn apply(
        &mut self,
        state: &mut EditorState,
        value: CatalogLabel,
    ) -> Result<(), Error> {
        let id = NodeId::new_opt(value.id).ok_or(Error::Precondition)?;
        let mut found = state
            .components
            .components
            .iter()
            .enumerate()
            .filter(|(_, component)| component.id == id);
        let (index, component) = found.next().ok_or(Error::Precondition)?;
        if found.next().is_some() || component.name != value.expected_name {
            return Err(Error::Precondition);
        }
        let mut name = value.name;
        state.components.exchange_name_at(index, &mut name);
        if !self.by_id.contains_key(&id) {
            self.by_id.insert(id.clone(), self.old.len());
            self.old.push(Old { index, id, name });
        }
        Ok(())
    }
    pub(super) fn restore(self, state: &mut EditorState) -> Result<(), Error> {
        let mut indices = Vec::with_capacity(self.old.len());
        for old in &self.old {
            let mut found = state
                .components
                .components
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.id == old.id);
            let (index, _) = found.next().ok_or(Error::Rollback)?;
            if found.next().is_some() {
                return Err(Error::Rollback);
            }
            indices.push(index);
        }
        for (mut old, index) in self.old.into_iter().zip(indices).rev() {
            state.components.exchange_name_at(index, &mut old.name);
        }
        Ok(())
    }
    pub(super) fn compact(
        &self,
        state: &EditorState,
    ) -> Result<Vec<auxiliary::Patch>, ScopedHistoryError> {
        let mut patches = Vec::with_capacity(self.old.len());
        for old in &self.old {
            let current = state
                .components
                .components
                .get(old.index)
                .filter(|component| component.id == old.id)
                .ok_or(ScopedHistoryError::NodePathChanged)?;
            if current.name != old.name {
                patches.push(auxiliary::Patch::Component {
                    id: old.id.clone(),
                    old: old.name.clone(),
                    expected: current.name.clone(),
                });
            }
        }
        Ok(patches)
    }
}
