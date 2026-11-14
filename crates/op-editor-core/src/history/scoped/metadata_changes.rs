//! Read-only exact header changes from a completed trusted target transaction.
use super::{patch, ScopedEditorSnapshot};
use crate::{EditorState, PenNodeExt};
use jian_ops_schema::node::PenNode;

impl ScopedEditorSnapshot {
    pub(crate) fn visit_metadata_changes(
        &self,
        state: &EditorState,
        visitor: &mut impl FnMut(&PenNode, &PenNode),
    ) -> bool {
        if self.header.is_some()
            || !self.page_headers.is_empty()
            || self.validate_shape(state).is_err()
        {
            return false;
        }
        // Validate the complete shape before exposing any partial delta. The
        // caller must discard accumulated output when this method returns false.
        let mut visited = std::collections::HashSet::new();
        for edit in &self.patches {
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
