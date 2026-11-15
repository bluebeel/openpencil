//! UI ref ownership is part of an exact variable-rewrite receipt. Ordinary
//! snapshots retain their established complete cache exchange behavior.
use super::*;
impl ScopedEditorSnapshot {
    pub(super) fn owns_ui_ref_receipt(&self) -> bool {
        self.auxiliary_patches
            .iter()
            .any(auxiliary::Patch::owns_ui_refs)
    }
    pub(super) fn release_owned_ui_refs(&mut self) {
        if self.owns_ui_ref_receipt() {
            self.fill_refs = HashMap::new();
            self.stroke_refs = HashMap::new();
        }
    }
    pub(super) fn exchange_legacy_ui_refs(
        &mut self,
        state: &mut EditorState,
        fill: HashMap<NodeId, String>,
        stroke: HashMap<NodeId, String>,
    ) {
        if !self.owns_ui_ref_receipt() {
            state.ui.variables.fill_refs = mem::replace(&mut self.fill_refs, fill);
            state.ui.variables.stroke_refs = mem::replace(&mut self.stroke_refs, stroke);
        }
    }
}
