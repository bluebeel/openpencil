//! Scoped pending restore; moved without behavior changes before upsert integration.
use super::*;
impl PendingScopedSnapshot {
    pub(crate) fn restore(&mut self, state: &mut EditorState) -> Result<(), ScopedHistoryError> {
        if self
            .auxiliary
            .as_ref()
            .is_some_and(auxiliary::Pending::is_upsert)
        {
            return self.restore_upsert(state);
        }
        self.metadata.validate_shape(state)?;
        for forest in mem::take(&mut self.forests) {
            match forest {
                CapturedForest::Root(nodes) => state.doc.children = nodes,
                CapturedForest::Regions(regions) => regions.restore(state)?,
                CapturedForest::Page { index, page } => {
                    state.doc.pages.as_mut().unwrap()[index] = page
                }
            }
        }
        targets::restore(state, std::mem::take(&mut self.targets))?;
        if let Some(clones) = self.clones.take() {
            clones.restore(state)?;
        }
        if let Some(boolean) = self.boolean.take() {
            boolean.restore(state)?;
        }
        if let Some(deletion) = self.deletion.take() {
            deletion.restore(state)?;
        }
        if let Some(auxiliary) = self.auxiliary.take() {
            auxiliary.restore(state)?;
        }
        if let Some(header) = self.header.take() {
            header.restore(&mut state.doc);
        }
        state.selection = self.metadata.selection.clone();
        state.ui.active_page_index = self.metadata.active_page_index;
        if self.metadata.captures_components {
            state.components = self.metadata.components.materialize();
        }
        state.app_state_owner = self.metadata.app_state_owner.clone();
        state.ui.variables.fill_refs = self.metadata.fill_refs.clone();
        state.ui.variables.stroke_refs = self.metadata.stroke_refs.clone();
        state.editor_ui.preserve_authored_geometry = self.metadata.preserve_authored_geometry;
        state.revision = self.metadata.revision;
        state.sync_dirty_flag();
        Ok(())
    }
}
