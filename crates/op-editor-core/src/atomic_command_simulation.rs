//! One owned simulation with exactly one Batch allocator for the whole plan.

use super::AtomicCommandAllocator;
use crate::{EditorCommand, EditorState, IdAllocError};

pub struct AtomicCommandSimulation {
    state: EditorState,
    allocator: AtomicCommandAllocator,
}

impl AtomicCommandSimulation {
    pub fn new(source: &EditorState) -> Self {
        let allocator = AtomicCommandAllocator::new(source);
        let mut state = source.clone_for_command_simulation();
        // Reads observe the pre-transaction history, not the empty scratch
        // history of the DSL simulator. Undo capture stays suspended, so
        // staged commands do not invent intermediate history entries.
        state.history = source.history.clone();
        Self { allocator, state }
    }

    /// The caller has already checked supports_atomic_batch. Keep the allocator
    /// cursor across deletion and authored-id steps just as the final Batch
    /// does, rather than re-seeding from each intermediate document.
    pub fn apply(&mut self, command: EditorCommand) -> Result<bool, IdAllocError> {
        self.allocator.apply(&mut self.state, command)
    }
}

impl std::ops::Deref for AtomicCommandSimulation {
    type Target = EditorState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}
