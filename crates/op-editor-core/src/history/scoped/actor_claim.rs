//! Conservative, allocation-free overlap proofs for selective exchanges.
use super::{
    patch::{Patch, Scope},
    ScopedEditorSnapshot, ScopedHistoryError,
};
use crate::EditorState;
#[derive(Clone, Copy)]
pub(super) enum Kind {
    Header,
    Subtree,
    Children,
}
#[derive(Clone, Copy)]
pub(super) struct Region<'a> {
    pub scope: &'a Scope,
    pub path: &'a [usize],
    pub kind: Kind,
}
impl Region<'_> {
    fn intersects(self, other: Region<'_>) -> bool {
        if self.scope != other.scope {
            return false;
        }
        let covers = |a: Region<'_>, b: Region<'_>| match a.kind {
            Kind::Header => a.path == b.path && !matches!(b.kind, Kind::Children),
            Kind::Subtree => b.path.starts_with(a.path),
            // Parent header swaps also validate child-slot presence. A list
            // change is conservatively overlapping at that exact parent too.
            Kind::Children => b.path.starts_with(a.path),
        };
        covers(self, other) || covers(other, self)
    }
}
impl Patch {
    fn regions(&self) -> [Option<Region<'_>>; 2] {
        let region = match self {
            Self::Node(p) => Region {
                scope: &p.scope,
                path: &p.path,
                kind: if p.metadata_only {
                    Kind::Header
                } else {
                    Kind::Subtree
                },
            },
            Self::Forest(p) => Region {
                scope: &p.scope,
                path: &p.parent,
                kind: Kind::Children,
            },
            Self::Order(p) => Region {
                scope: &p.scope,
                path: &p.parent,
                kind: Kind::Children,
            },
            Self::Splice(p) => Region {
                scope: &p.scope,
                path: &p.parent,
                kind: Kind::Children,
            },
            Self::Topology(p) => return p.actor_regions(),
        };
        [Some(region), None]
    }
}
impl ScopedEditorSnapshot {
    pub(crate) fn actor_conflicts(&self, other: &Self) -> bool {
        if !self.actor_safe || !other.actor_safe {
            return true;
        }
        self.patches.iter().any(|a| {
            other.patches.iter().any(|b| {
                a.regions()
                    .into_iter()
                    .flatten()
                    .any(|a| b.regions().into_iter().flatten().any(|b| a.intersects(b)))
            })
        })
    }
    /// Selective exchanges touch canonical patches and the current actor's UI.
    /// Shared component/reference caches and other actors' state stay current.
    pub(crate) fn exchange_actor(
        &mut self,
        state: &mut EditorState,
    ) -> Result<(), ScopedHistoryError> {
        self.validate_shape(state)?;
        if !self.actor_safe {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        for patch in &self.patches {
            patch.validate(&state.doc)?;
        }
        let mut reordered = super::patch::Reorders::default();
        for patch in &mut self.patches {
            patch.exchange(&mut state.doc, &mut reordered);
        }
        for patch in &mut self.patches {
            patch.rebase(&reordered);
        }
        std::mem::swap(&mut self.selection, &mut state.selection);
        std::mem::swap(&mut self.active_page_index, &mut state.ui.active_page_index);
        self.revision = state.revision;
        state.mark_document_changed();
        Ok(())
    }
}
