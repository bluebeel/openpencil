//! Read-only exact header changes from a completed trusted target transaction.
use super::{patch, ScopedEditorSnapshot};
use crate::{EditorState, PenNodeExt};
use jian_ops_schema::node::PenNode;

impl ScopedEditorSnapshot {
    pub(crate) fn visit_canonical_edits<'a>(
        &'a self,
        state: &'a EditorState,
        limit: usize,
        nodes: &mut impl FnMut(&PenNode, &PenNode),
        headers: &mut impl FnMut(crate::CanonicalHeaderChange) -> bool,
        appends: &mut impl FnMut(crate::CanonicalAppend<'a>) -> bool,
    ) -> bool {
        if self.page_patch.is_some()
            || self.header.is_some()
            || !self.page_headers.is_empty()
            || self.validate_shape(state).is_err()
            || super::auxiliary::Prepared::capture(state, &self.auxiliary_patches, false).is_err()
            || self
                .patches
                .iter()
                .flat_map(patch::Patch::splices)
                .any(|splice| splice.canonical_append(&state.doc).is_none())
        {
            return false;
        }
        if !self.visit_nodes(state, nodes, true) {
            return false;
        }
        for splice in self.patches.iter().flat_map(patch::Patch::splices) {
            let Some(receipt) = splice.canonical_append(&state.doc) else {
                return false;
            };
            if !appends(receipt) {
                return false;
            }
        }
        self.auxiliary_patches
            .iter()
            .all(|patch| patch.visit_canonical_headers(state, limit, headers))
    }

    pub(crate) fn visit_canonical_changes(
        &self,
        state: &EditorState,
        limit: usize,
        node: &mut impl FnMut(&PenNode, &PenNode),
        header: &mut impl FnMut(crate::CanonicalHeaderChange) -> bool,
    ) -> bool {
        if self.page_patch.is_some()
            || self.header.is_some()
            || !self.page_headers.is_empty()
            || self.validate_shape(state).is_err()
        {
            return false;
        }
        if super::auxiliary::Prepared::capture(state, &self.auxiliary_patches, false).is_err()
            || !self.visit_nodes(state, node, false)
        {
            return false;
        }
        self.auxiliary_patches
            .iter()
            .all(|patch| patch.visit_canonical_headers(state, limit, header))
    }
    pub(crate) fn visit_metadata_changes(
        &self,
        state: &EditorState,
        visitor: &mut impl FnMut(&PenNode, &PenNode),
    ) -> bool {
        if self.page_patch.is_some()
            || self
                .auxiliary_patches
                .iter()
                .any(super::auxiliary::Patch::changes_document_outside_nodes)
            || self.header.is_some()
            || !self.page_headers.is_empty()
            || self.validate_shape(state).is_err()
        {
            return false;
        }
        self.visit_nodes(state, visitor, false)
    }
    fn visit_nodes(
        &self,
        state: &EditorState,
        visitor: &mut impl FnMut(&PenNode, &PenNode),
        allow_appends: bool,
    ) -> bool {
        // Validate the complete shape before exposing any partial delta. The
        // caller must discard accumulated output when this method returns false.
        let mut visited = std::collections::HashSet::new();
        for edit in &self.patches {
            if allow_appends && edit.splices().next().is_some() {
                continue;
            }
            let patch::Patch::Node(p) = edit else {
                return false;
            };
            if !p.metadata_only
                || edit.validate(&state.doc).is_err()
                || !visited.insert((&p.scope, p.path.as_ref()))
            {
                return false;
            }
        }
        for edit in &self.patches {
            if allow_appends && edit.splices().next().is_some() {
                continue;
            }
            let patch::Patch::Node(p) = edit else {
                return false;
            };
            let Ok(roots) = patch::roots(&state.doc, &p.scope) else {
                return false;
            };
            let Some(current) = patch::node(roots, &p.path) else {
                return false;
            };
            if p.old.children().is_some() != current.children().is_some() {
                return false;
            }
            visitor(p.old.metadata(), current);
        }
        true
    }
}
