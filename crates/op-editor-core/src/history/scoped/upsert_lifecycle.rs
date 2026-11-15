//! Prepared conversion execution and exact inverse installation are owned by
//! the normal scoped transaction, including failed-save/panic rollback.
use super::*;
impl PendingScopedSnapshot {
    pub(crate) fn apply_prepared_upsert(
        &mut self,
        state: &mut EditorState,
        command: &crate::EditorCommand,
    ) -> Option<bool> {
        self.auxiliary.as_mut()?.apply_upsert(state, command)
    }
    fn prepare_upsert_compaction(&mut self, state: &EditorState) -> Result<(), ScopedHistoryError> {
        if let Some(pending) = &self.auxiliary {
            pending.prepare_page(state, &mut self.metadata)?;
        }
        Ok(())
    }
    fn restore_upsert(&mut self, state: &mut EditorState) -> Result<(), ScopedHistoryError> {
        self.prepare_upsert_compaction(state)?;
        self.metadata.validate_shape(state)?;
        // Validate/compact all destinations before exchange writes any inverse.
        // A partially applied prepared root owns only these exact regions.
        self.metadata
            .patches
            .extend(targets::compact(state, mem::take(&mut self.targets))?);
        if let Some(pending) = self.auxiliary.take() {
            pending.compact_into(state, &mut self.metadata.auxiliary_patches)?;
        }
        self.metadata.exchange(state)
    }
}
