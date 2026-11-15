use super::*;
use crate::{CanonicalHeaderChange, CanonicalHeaderField as Field};
impl Patch {
    pub(in crate::history::scoped::auxiliary) fn visit_canonical_headers(
        &self,
        state: &EditorState,
        limit: usize,
        visitor: &mut impl FnMut(CanonicalHeaderChange) -> bool,
    ) -> bool {
        if !self.validate(state, false) {
            return false;
        }
        if let Some(p) = &self.variables {
            if !p
                .canonical_change(Field::Variables, &state.doc.variables, limit)
                .is_some_and(&mut *visitor)
            {
                return false;
            }
        }
        if let Some(p) = &self.themes {
            if !p
                .canonical_change(Field::Themes, &state.doc.themes, limit)
                .is_some_and(&mut *visitor)
            {
                return false;
            }
        }
        if let Some(p) = &self.design_md {
            if !p
                .canonical_change(Field::DesignMd, &state.doc.design_md, limit)
                .is_some_and(&mut *visitor)
            {
                return false;
            }
        }
        if let Some(p) = &self.conversion {
            if !p
                .canonical_change(&state.doc, limit)
                .is_some_and(&mut *visitor)
            {
                return false;
            }
        }
        true // transient theme pins are not canonical document fields
    }
}
