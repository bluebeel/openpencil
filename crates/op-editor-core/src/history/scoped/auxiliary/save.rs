//! Only completed, validated canonical fields are exposed to a byte writer.
use super::*;
use crate::{CanonicalHeaderChange, CanonicalHeaderField as Field};
impl Patch {
    pub(in crate::history::scoped) fn visit_canonical_headers(
        &self,
        state: &EditorState,
        limit: usize,
        visitor: &mut impl FnMut(CanonicalHeaderChange) -> bool,
    ) -> bool {
        match self {
            Self::Bulk(p) => p.visit_canonical_headers(state, limit, visitor),
            Self::Variable(p) => p
                .canonical_change(Field::Variables, &state.doc.variables, limit)
                .is_some_and(visitor),
            Self::VariableRewrite(p) => p
                .variables()
                .canonical_change(Field::Variables, &state.doc.variables, limit)
                .is_some_and(visitor),
            Self::ConversionUnit(p) => p.canonical_change(&state.doc, limit).is_some_and(visitor),
            Self::Component { .. } | Self::Catalog(_) => true, // editorMeta is always independently re-encoded
            Self::Page { .. } => false,
        }
    }
}
