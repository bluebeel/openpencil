//! Direct audited component methods share the trusted compact inverse.
use super::*;
impl EditorState {
    pub(crate) fn finish_component_history(
        &mut self,
        pending: Option<PendingScopedSnapshot>,
        fallback: Option<EditorSnapshot>,
    ) {
        if let Some(pending) = pending {
            let snapshot = pending
                .compact(self)
                .expect("audited component mutation preserved its captured footprint");
            if snapshot.has_canonical_changes() {
                self.history_push_entry(HistoryEntry::Scoped(Arc::new(snapshot)));
            } else {
                self.mark_document_changed();
            }
        } else {
            self.push_undo(fallback);
        }
    }
}

impl PendingScopedSnapshot {
    pub(crate) fn capture_auxiliary(
        state: &EditorState,
        command: &crate::EditorCommand,
    ) -> Option<Self> {
        let parts = auxiliary::Pending::capture(state, command)?;
        Self::capture_auxiliary_parts(state, parts)
    }
    pub(crate) fn capture_component_detach(
        state: &EditorState,
        id: &NodeId,
        path: &[usize],
    ) -> Option<Self> {
        let parts = auxiliary::Pending::capture_detach(state, id, path)?;
        Self::capture_auxiliary_parts(state, parts)
    }
    fn capture_auxiliary_parts(
        state: &EditorState,
        (auxiliary, scopes, capture_components, target_specs): (
            auxiliary::Pending,
            Vec<Scope>,
            bool,
            Vec<TargetSpec>,
        ),
    ) -> Option<Self> {
        Some(Self {
            metadata: ScopedEditorSnapshot::auxiliary_with_components(
                state,
                scopes,
                capture_components,
            ),
            forests: Vec::new(),
            targets: targets::capture(state, target_specs).ok()?,
            targeted: true,
            header: None,
            clones: None,
            boolean: None,
            deletion: None,
            auxiliary: Some(auxiliary),
        })
    }
}
