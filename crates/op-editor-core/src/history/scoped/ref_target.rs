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

impl ScopedEditorSnapshot {
    pub(crate) fn visit_canonical_ref_targets<'a>(
        &'a self,
        state: &'a EditorState,
        limit: usize,
        visitor: &mut impl FnMut(crate::CanonicalRefTarget<'a>) -> bool,
    ) -> bool {
        if self.captures_components
            || self.page_patch.is_some()
            || self.header.is_some()
            || !self.page_headers.is_empty()
            || !self.auxiliary_patches.is_empty()
            || self.validate_shape(state).is_err()
        {
            return false;
        }
        let [edit @ patch::Patch::Node(p)] = self.patches.as_slice() else {
            return false;
        };
        if !p.metadata_only || edit.validate(&state.doc).is_err() {
            return false;
        }
        let Ok(roots) = patch::roots(&state.doc, &p.scope) else {
            return false;
        };
        let Some(current) = patch::node(roots, &p.path) else {
            return false;
        };
        let (PenNode::Ref(old_ref), PenNode::Ref(current_ref)) = (p.old.metadata(), current) else {
            return false;
        };
        if p.old.children().is_some() != current.children().is_some()
            || crate::history_snapshot::command_capture_node_bytes(current, true, limit).is_err()
            || crate::history_snapshot::command_capture_node_bytes(p.old.metadata(), true, limit)
                .is_err()
        {
            return false;
        }
        // The completed target capture already proved unchanged child wire.
        // Normalize only the one changed target before schema-complete exact
        // comparison (float bits and authored map order included). No subtree
        // or catalog materialization is needed, even for a Ref with children.
        let mut normalized = crate::pen_node_ext::clone_without_children(current);
        let PenNode::Ref(reference) = &mut normalized else {
            return false;
        };
        reference.target.clone_from(&old_ref.target);
        if !p.old.equals_header(&normalized) {
            return false;
        }
        visitor(crate::CanonicalRefTarget::certified(
            p.old.metadata(),
            current,
            &old_ref.target,
            &current_ref.target,
        ))
    }
}
