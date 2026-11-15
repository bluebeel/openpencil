//! Consume the completed Core proof immediately under its original writer.
use super::*;
impl SaveChain {
    pub(crate) fn observe_capture(
        &mut self,
        state: &EditorState,
        capture: Option<&op_editor_core::CommandEditCapture>,
    ) {
        let Some(capture) = capture else {
            self.invalidate();
            return;
        };
        let Some(base) = self.base.as_mut() else {
            return;
        };
        if !base.owner.ptr_eq(&state.derived_cache_owner())
            || base.generation != state.document_generation()
        {
            self.invalidate();
            return;
        }
        let mut visited = false;
        let exact = capture.visit_canonical_ref_targets(state, HEADER_VALUE_LIMIT, |receipt| {
            visited = true;
            receipt.old().id_str() == receipt.current().id_str()
                && base.patch_metadata(receipt.old(), receipt.current(), true).is_some()
        });
        if visited || exact {
            if !exact {
                self.invalidate();
            }
            return;
        }
        self.observe_deltas(state, |nodes, headers, splices| {
            capture.visit_canonical_deltas(state, HEADER_VALUE_LIMIT, nodes, headers, splices)
        });
    }
}
