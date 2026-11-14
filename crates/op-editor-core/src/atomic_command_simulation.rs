//! One owned simulation with exactly one Batch allocator for the whole plan.

use super::{IdFloor, ScopedIdAllocator};
use crate::{DocumentIdAllocator, EditorCommand, EditorState, IdAllocError};

pub struct AtomicCommandSimulation {
    state: EditorState,
    allocator: DocumentIdAllocator,
    floor: IdFloor,
    seed_error: Option<IdAllocError>,
}

impl AtomicCommandSimulation {
    pub fn new(source: &EditorState) -> Self {
        let (allocator, seed_error) =
            match DocumentIdAllocator::sequential_for_document(&source.doc) {
                Ok(allocator) => (allocator, None),
                Err(error) => (DocumentIdAllocator::sequential(1), Some(error)),
            };
        Self {
            allocator,
            seed_error,
            state: source.clone_for_command_simulation(),
            floor: IdFloor::default(),
        }
    }

    /// The caller has already checked supports_atomic_batch. Keep the allocator
    /// cursor across deletion and authored-id steps just as the final Batch
    /// does, rather than re-seeding from each intermediate document.
    pub fn apply(&mut self, command: EditorCommand) -> Result<bool, IdAllocError> {
        if crate::command_apply_legacy::allocation_policy::requires_id_seed(&command) {
            if let Some(error) = self.seed_error {
                return Err(error);
            }
        }
        let mut allocator = ScopedIdAllocator {
            inner: &mut self.allocator,
            floor: std::mem::take(&mut self.floor),
        };
        allocator.prepare(&command);
        let result = self.state.apply_with_allocator(command, &mut allocator);
        self.floor = allocator.floor;
        result
    }
}

impl std::ops::Deref for AtomicCommandSimulation {
    type Target = EditorState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}
