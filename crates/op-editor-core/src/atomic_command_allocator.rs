//! One sequential cursor and exact authored-ID floor for a command sequence.
use super::{IdFloor, ScopedIdAllocator};
use crate::{DocumentIdAllocator, EditorCommand, EditorState, IdAllocError, IdAllocator, NodeId};
use jian_ops_schema::PenDocument;
use std::collections::HashSet;

pub(crate) struct AtomicCommandAllocator {
    allocator: DocumentIdAllocator,
    floor: IdFloor,
    seed_error: Option<IdAllocError>,
    collision_proof: bool,
}
impl AtomicCommandAllocator {
    pub(crate) fn new(source: &EditorState) -> Self {
        let (allocator, seed_error) = match source.standalone_id_allocator() {
            Ok(allocator) => (allocator, None),
            Err(error) => (DocumentIdAllocator::sequential(1), Some(error)),
        };
        Self {
            allocator,
            collision_proof: seed_error.is_none(),
            seed_error,
            floor: IdFloor::default(),
        }
    }
    pub(crate) fn apply(
        &mut self,
        state: &mut EditorState,
        command: EditorCommand,
    ) -> Result<bool, IdAllocError> {
        self.apply_with_identity_proof(state, command, false)
    }
    /// The additional proof is produced by strict physical target/splice
    /// classification, never by a caller-owned revision or public document.
    pub(crate) fn apply_with_identity_proof(
        &mut self,
        state: &mut EditorState,
        command: EditorCommand,
        preserves_cursor: bool,
    ) -> Result<bool, IdAllocError> {
        let needs_ids =
            crate::command_apply_legacy::allocation_policy::requires_id_seed_in(state, &command);
        if needs_ids {
            if let Some(error) = self.seed_error {
                return Err(error);
            }
        }
        // Only generated numeric IDs, deletion, or proved identity-preserving
        // edits keep an exact cursor. Unknown authored mutations invalidate it
        // BEFORE execution. Thereafter retain complete collision sets without
        // reseeding: a Batch cursor must keep its historical candidate order
        // even when an authored subtree introduces a much larger numeric ID.
        let preserves_cursor = preserves_cursor
            || !command.may_change_document_identities()
            || matches!(
                command,
                EditorCommand::InsertNode { .. }
                    | EditorCommand::DeleteNode { .. }
                    | EditorCommand::ReplaceNode { .. }
            );
        self.collision_proof &= preserves_cursor;
        let mut cursor = ExclusiveCursor {
            inner: &mut self.allocator,
            proof: self.collision_proof,
        };
        let mut allocator = ScopedIdAllocator {
            inner: &mut cursor,
            floor: std::mem::take(&mut self.floor),
        };
        allocator.prepare(&command);
        let result = state.apply_with_allocator(command, &mut allocator);
        self.floor = allocator.floor;
        result
    }
}

/// The sequential cursor was seeded from this exact exclusive authority and
/// every intervening ID mutation was observed. Fresh candidates cannot collide
/// with any earlier document/page ID; each local set still reserves the IDs
/// emitted by one subtree remap. Custom allocators keep their complete default.
struct ExclusiveCursor<'a> {
    inner: &'a mut DocumentIdAllocator,
    proof: bool,
}
impl IdAllocator for ExclusiveCursor<'_> {
    fn document_collision_ids(&self, document: &PenDocument) -> HashSet<NodeId> {
        if self.proof {
            HashSet::new()
        } else {
            self.inner.document_collision_ids(document)
        }
    }
    fn authored_id_floor(&mut self, document: &PenDocument) -> Option<u64> {
        self.inner.authored_id_floor(document)
    }
    fn authored_ids_inserted(&mut self, largest: Option<u64>) {
        self.inner.authored_ids_inserted(largest);
    }
    fn allocate(&mut self, taken: &mut HashSet<NodeId>) -> Result<NodeId, IdAllocError> {
        self.inner.allocate(taken)
    }
}
