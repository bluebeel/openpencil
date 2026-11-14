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
    pub(super) fn regions(&self) -> [Option<Region<'_>>; 2] {
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
            Self::Clones(_) => return [None, None],
        };
        [Some(region), None]
    }
}
impl Patch {
    pub(super) fn actor_overlaps(&self, other: &Self) -> bool {
        let mut own = self.splices().peekable();
        if own.peek().is_some() {
            return own.any(|splice| splice.conflicts_patch(other));
        }
        let mut foreign = other.splices().peekable();
        if foreign.peek().is_some() {
            return foreign.any(|splice| splice.conflicts_patch(self));
        }
        match (self, other) {
            (Self::Node(a), Self::Node(b)) => {
                if a.scope != b.scope {
                    return false;
                }
                a.overlaps_affected(b)
            }
            (Self::Topology(topology), Self::Node(node))
            | (Self::Node(node), Self::Topology(topology)) => topology.conflicts_node(node),
            _ => self.regions().into_iter().flatten().any(|a| {
                other
                    .regions()
                    .into_iter()
                    .flatten()
                    .any(|b| a.intersects(b))
            }),
        }
    }
}
impl ScopedEditorSnapshot {
    pub(crate) fn actor_conflicts(&self, other: &Self) -> bool {
        !self.actor_safe
            || !other.actor_safe
            || self
                .auxiliary_patches
                .iter()
                .any(|a| other.auxiliary_patches.iter().any(|b| a.actor_conflicts(b)))
            || self
                .patches
                .iter()
                .any(|a| other.patches.iter().any(|b| a.actor_overlaps(b)))
    }
    /// Selective exchanges touch canonical patches and the current actor's UI.
    /// Shared component/reference caches and other actors' state stay current.
    pub(crate) fn exchange_actor(
        &mut self,
        state: &mut EditorState,
    ) -> Result<(), ScopedHistoryError> {
        self.exchange_actor_inner(state, false)
    }
    pub(crate) fn exchange_actor_rollback(
        &mut self,
        state: &mut EditorState,
    ) -> Result<(), ScopedHistoryError> {
        self.exchange_actor_inner(state, true)
    }
    fn exchange_actor_inner(
        &mut self,
        state: &mut EditorState,
        rollback: bool,
    ) -> Result<(), ScopedHistoryError> {
        self.validate_shape(state)?;
        if !self.actor_safe {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let auxiliary = super::auxiliary::Prepared::capture(state, &self.auxiliary_patches, true)?;
        super::actor_rebase::paths(&state.doc, &mut self.patches)?;
        if !rollback {
            super::actor_rebase::restored_node_collisions(&state.doc, &self.patches)?;
        }
        let catalog = super::splice_exchange::Catalog::new(&state.history, &self.patches);
        let splices = super::splice_exchange::Prepared::capture(
            &state.doc,
            &self.patches,
            &catalog,
            !rollback,
        )?;
        if let Some(prepared) = &splices {
            prepared.rebase(&mut self.patches);
        }
        for patch in &self.patches {
            patch.validate(&state.doc)?;
        }
        let mut reordered = super::patch::Reorders::default();
        if let Some(splices) = splices {
            splices.exchange(&mut state.doc, &mut self.patches);
        } else {
            for patch in &mut self.patches {
                patch.exchange(&mut state.doc, &mut reordered);
            }
            for patch in &mut self.patches {
                patch.rebase(&reordered);
            }
        }
        auxiliary.exchange(state, &mut self.auxiliary_patches);
        std::mem::swap(&mut self.selection, &mut state.selection);
        std::mem::swap(&mut self.active_page_index, &mut state.ui.active_page_index);
        self.revision = state.revision;
        state.mark_document_changed();
        Ok(())
    }
}
