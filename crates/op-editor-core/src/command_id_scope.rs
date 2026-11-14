//! Numeric-id proofs owned by one exclusive command sequence.
//!
//! No proof is attached to a public mutable document or revision. Deletions
//! may leave an upper bound; anything that can introduce unobserved ids drops
//! it. Non-numeric authored ids still use the ordinary complete collision set.

use crate::{DocumentIdAllocator, EditorCommand, EditorState, IdAllocError, IdAllocator, NodeId};
use jian_ops_schema::PenDocument;
use std::collections::HashSet;

#[derive(Default)]
struct IdFloor(Option<u64>);

impl IdFloor {
    fn prepare(&mut self, command: &EditorCommand) {
        use EditorCommand as C;
        if !matches!(
            command,
            C::InsertAuthoredSubtree { .. }
                | C::InsertAuthoredSubtreePreservingRoots { .. }
                | C::UpdateNode { .. }
                | C::MoveNode { .. }
                | C::DeleteNode { .. }
                | C::SetActivePage { .. }
        ) {
            self.0 = None;
        }
    }

    fn get(&mut self, document: &PenDocument) -> Option<u64> {
        if self.0.is_none() {
            self.0 = crate::id_allocator::next_sequential_counter(document).ok();
        }
        self.0
    }

    fn inserted(&mut self, largest: Option<u64>) {
        if let (Some(floor), Some(largest)) = (self.0, largest) {
            self.0 = largest.checked_add(1).map(|next| floor.max(next));
        }
    }
}

pub(crate) struct ScopedIdAllocator<'a> {
    inner: &'a mut dyn IdAllocator,
    floor: IdFloor,
}

impl<'a> ScopedIdAllocator<'a> {
    pub(crate) fn new(inner: &'a mut dyn IdAllocator) -> Self {
        Self {
            inner,
            floor: IdFloor::default(),
        }
    }

    pub(crate) fn prepare(&mut self, command: &EditorCommand) {
        self.floor.prepare(command);
    }
}

impl IdAllocator for ScopedIdAllocator<'_> {
    fn document_collision_ids(&self, document: &PenDocument) -> HashSet<NodeId> {
        self.inner.document_collision_ids(document)
    }

    fn authored_id_floor(&mut self, document: &PenDocument) -> Option<u64> {
        self.floor.get(document)
    }

    fn authored_ids_inserted(&mut self, largest: Option<u64>) {
        self.floor.inserted(largest);
    }

    fn allocate(&mut self, taken: &mut HashSet<NodeId>) -> Result<NodeId, IdAllocError> {
        self.inner.allocate(taken)
    }
}

/// An owned command simulation with a sequence-local numeric-id proof.
///
/// Only shared access to the editor is exposed. Consuming this wrapper drops
/// its proof before callers regain mutable access to the public document.
pub struct CommandSimulation {
    state: EditorState,
    floor: IdFloor,
}

impl CommandSimulation {
    pub fn new(state: EditorState) -> Self {
        Self {
            state,
            floor: IdFloor::default(),
        }
    }

    pub fn into_inner(self) -> EditorState {
        self.state
    }

    /// Preserve the program's historical max(current seed, minted high-water)
    /// rule. A conservative cached floor is usable only when the program has
    /// already reserved every numeric candidate below that bound.
    pub fn next_program_id_seed(&mut self, minted_high_water: u64) -> Option<u64> {
        if self.floor.0.is_some_and(|floor| floor <= minted_high_water) {
            return Some(minted_high_water);
        }
        self.floor.0 = crate::id_allocator::next_sequential_counter(&self.state.doc).ok();
        self.floor.0.map(|floor| floor.max(minted_high_water))
    }

    pub fn apply(&mut self, command: EditorCommand) -> bool {
        self.floor.prepare(&command);
        let mut allocator =
            if crate::command_apply_legacy::allocation_policy::requires_id_seed(&command) {
                let Ok(allocator) = DocumentIdAllocator::sequential_for_document(&self.state.doc)
                else {
                    return false;
                };
                allocator
            } else {
                DocumentIdAllocator::sequential(1)
            };
        let mut scoped = ScopedIdAllocator {
            inner: &mut allocator,
            floor: std::mem::take(&mut self.floor),
        };
        // The wrapper is owned scratch state. On unwind it cannot expose a
        // proof to any later external mutation; the atomic Batch owns rollback.
        let changed = self
            .state
            .apply_with_allocator(command, &mut scoped)
            .unwrap_or(false);
        self.floor = scoped.floor;
        changed
    }
}

impl std::ops::Deref for CommandSimulation {
    type Target = EditorState;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

#[cfg(test)]
#[path = "command_id_scope_tests.rs"]
mod tests;
