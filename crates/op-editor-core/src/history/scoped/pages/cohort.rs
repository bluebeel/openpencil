//! Additional live page-table receipts for bounded Undo/Redo cohorts.
use super::{forecast, Kind, Patch};
use crate::EditorState;
use std::{collections::HashSet, mem};
impl Patch {
    pub(in crate::history::scoped) fn cohort_live_bound(
        &self,
        state: &EditorState,
        limit: usize,
    ) -> Option<usize> {
        let mut bytes = 128usize * 1024;
        let pages = state.doc.pages.as_ref();
        bytes = bytes.checked_add(
            pages
                .map_or(0, Vec::capacity)
                .checked_mul(mem::size_of::<jian_ops_schema::page::PenPage>() * 8 + 1024)?,
        )?;
        let expected = match &self.kind {
            Kind::Splice { expected, .. } => {
                Some(expected.iter().map(String::as_str).collect::<HashSet<_>>())
            }
            _ => None,
        };
        for page in pages.into_iter().flatten() {
            bytes = bytes.checked_add(page.id.len().checked_mul(64)?)?;
            if expected
                .as_ref()
                .is_some_and(|ids| ids.contains(page.id.as_str()))
            {
                bytes = bytes.checked_add(
                    forecast::page_header(page, limit.checked_sub(bytes)?)?.checked_mul(4)?,
                )?;
                bytes = bytes.checked_add(
                    forecast::forest(&page.children, limit.checked_sub(bytes)?)?
                        .0
                        .checked_mul(2)?,
                )?;
            }
        }
        if self
            .migration
            .as_ref()
            .is_some_and(|migration| !migration.routes.is_empty())
        {
            let backing = state.components.document_backing();
            bytes = bytes.checked_add(backing.capacity().checked_mul(
                mem::size_of::<(
                    crate::NodeId,
                    crate::component_backing::DocumentNodeLocation,
                )>() + 64,
            )?)?;
            for (id, location) in backing.iter() {
                bytes = bytes
                    .checked_add(id.as_str().len().checked_mul(8)?)?
                    .checked_add(
                        location
                            .child_path
                            .len()
                            .checked_mul(mem::size_of::<usize>() * 8)?,
                    )?;
            }
        }
        (bytes <= limit).then_some(bytes)
    }
}
