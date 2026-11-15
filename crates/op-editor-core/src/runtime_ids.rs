//! One runtime numeric high-water per editor/file. Undo and deletion cannot
//! reuse automatic ids still named by another actor's retained redo branch.
use crate::{
    DocumentIdAllocator, EditorCommand, EditorSnapshot, EditorState, IdAllocError, IdAllocator,
    NodeId, SequentialIdAllocator,
};
use jian_ops_schema::PenDocument;
use std::collections::HashSet;
struct Observed<'a> {
    inner: &'a mut dyn IdAllocator,
    highest: u64,
}
impl IdAllocator for Observed<'_> {
    fn document_collision_ids(&self, document: &PenDocument) -> HashSet<NodeId> {
        self.inner.document_collision_ids(document)
    }
    fn authored_id_floor(&mut self, document: &PenDocument) -> Option<u64> {
        self.inner.authored_id_floor(document)
    }
    fn authored_ids_inserted(&mut self, largest: Option<u64>) {
        if let Some(largest) = largest {
            self.highest = self.highest.max(largest);
        }
        self.inner.authored_ids_inserted(largest);
    }
    fn allocate(&mut self, taken: &mut HashSet<NodeId>) -> Result<NodeId, IdAllocError> {
        let id = self.inner.allocate(taken)?;
        if let Some(number) = crate::walkers::parse_n_id(id.as_str()) {
            self.highest = self.highest.max(number);
        }
        Ok(id)
    }
}
impl EditorState {
    pub fn runtime_id_high_water(&self) -> u64 {
        self.runtime_node_high_water
    }
    pub(crate) fn record_runtime_id_high_water(&mut self, highest: u64) {
        self.runtime_node_high_water = self.runtime_node_high_water.max(highest);
    }
    pub(crate) fn record_runtime_next_id(&mut self, next: u64) {
        if next > 1 {
            self.record_runtime_id_high_water(next - 1);
        }
    }
    pub(crate) fn runtime_next_id_seed(&self) -> Result<u64, IdAllocError> {
        self.runtime_node_high_water
            .checked_add(1)
            .ok_or(IdAllocError::CounterExhausted)
    }
    pub fn standalone_next_id_seed(&self) -> Result<u64, IdAllocError> {
        if let Some(seed) = self.command_id_seed.get(self) {
            return seed;
        }
        let next = crate::id_allocator::next_sequential_counter(&self.doc)?;
        let runtime = self.runtime_next_id_seed()?;
        Ok(next.max(runtime))
    }
    pub fn standalone_id_allocator(&self) -> Result<DocumentIdAllocator, IdAllocError> {
        Ok(DocumentIdAllocator::sequential(
            self.standalone_next_id_seed()?,
        ))
    }
    pub fn sequential_id_allocator(
        &self,
        requested: u64,
    ) -> Result<SequentialIdAllocator, IdAllocError> {
        Ok(SequentialIdAllocator::new(
            requested.max(self.standalone_next_id_seed()?),
        ))
    }
    pub(crate) fn observe_runtime_allocations<T, E>(
        &mut self,
        allocator: &mut dyn IdAllocator,
        operation: impl FnOnce(&mut Self, &mut dyn IdAllocator) -> Result<T, E>,
        accepted: impl FnOnce(&T) -> bool,
    ) -> Result<T, E> {
        let outer = self.runtime_allocator_depth == 0;
        self.runtime_allocator_depth += 1;
        let mut observed = Observed {
            inner: allocator,
            highest: 0,
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            operation(self, &mut observed)
        }));
        self.runtime_allocator_depth -= 1;
        match result {
            Ok(result) => {
                if outer && result.as_ref().is_ok_and(accepted) {
                    self.record_runtime_id_high_water(observed.highest);
                }
                result
            }
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
    pub(crate) fn apply_with_allocator_checkpoint(
        &mut self,
        command: EditorCommand,
        allocator: &mut dyn IdAllocator,
        checkpoint: Option<&EditorSnapshot>,
    ) -> Result<bool, IdAllocError> {
        self.observe_command_id_sequence(&command);
        self.observe_runtime_allocations(
            allocator,
            |state, allocator| {
                state.apply_with_allocator_checkpoint_inner(command, allocator, checkpoint)
            },
            |changed| *changed,
        )
    }
}
#[path = "runtime_ids_assets.rs"]
mod assets;
#[path = "runtime_ids_nodes.rs"]
mod nodes;
#[path = "runtime_ids_pages.rs"]
mod pages;
