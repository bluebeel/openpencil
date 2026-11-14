//! Lexical Ref-target writes cannot mutate the independent component catalog.
use super::*;
impl PendingScopedSnapshot {
    pub(crate) fn capture_ref_target(
        state: &EditorState,
        scopes: Vec<Scope>,
        specs: Vec<TargetSpec>,
    ) -> Result<Self, ScopedHistoryError> {
        let [spec] = specs.as_slice() else {
            return Err(ScopedHistoryError::NodePathChanged);
        };
        if spec.subtree || spec.splice.is_some() || spec.topology.is_some() {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let target = patch::node(patch::roots(&state.doc, &spec.scope)?, &spec.path)
            .ok_or(ScopedHistoryError::NodePathChanged)?;
        let id = crate::NodeId::new_opt(crate::PenNodeExt::id_str(target))
            .ok_or(ScopedHistoryError::NodePathChanged)?;
        if !matches!(target, PenNode::Ref(_)) || state.components.is_document_backed(&id) {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        Self::capture_targets_with_components(state, scopes, false, specs, false)
    }
}
