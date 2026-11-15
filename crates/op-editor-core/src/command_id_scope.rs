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
        self.inner.authored_ids_inserted(largest);
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
    // Exact live numeric high-water, valid only for this exclusively owned
    // sequence. Deletions/unknown structural edits discard it before applying.
    sequential_seed: Option<u64>,
}

impl CommandSimulation {
    pub fn new(state: EditorState) -> Self {
        Self {
            state,
            floor: IdFloor::default(),
            sequential_seed: None,
        }
    }

    pub fn into_inner(self) -> EditorState {
        self.state
    }

    /// Preserve the program's historical max(current seed, minted high-water)
    /// rule. A conservative cached floor is usable only when the program has
    /// already reserved every numeric candidate below that bound.
    pub fn next_program_id_seed(&mut self, minted_high_water: u64) -> Option<u64> {
        let minted_high_water = minted_high_water.max(self.state.runtime_next_id_seed().ok()?);
        if let Some(seed) = self.sequential_seed {
            return Some(seed.max(minted_high_water));
        }
        if self.floor.0.is_some_and(|floor| floor <= minted_high_water) {
            return Some(minted_high_water);
        }
        self.floor.0 = self.state.standalone_next_id_seed().ok();
        self.floor.0.map(|floor| floor.max(minted_high_water))
    }

    pub fn apply(&mut self, command: EditorCommand) -> bool {
        let retain_seed = preserves_sequential_seed(&command);
        if !retain_seed {
            self.sequential_seed = None;
        }
        self.floor.prepare(&command);
        let allocates = crate::command_apply_legacy::allocation_policy::requires_id_seed_in(
            &self.state,
            &command,
        );
        let mut allocator = if allocates {
            let seed = match self.sequential_seed {
                Some(seed) => match self.state.runtime_next_id_seed() {
                    Ok(runtime) => seed.max(runtime),
                    Err(_) => return false,
                },
                None => {
                    let Ok(seed) = self.state.standalone_next_id_seed() else {
                        return false;
                    };
                    seed
                }
            };
            DocumentIdAllocator::sequential(seed)
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
        if allocates && changed && retain_seed {
            self.sequential_seed = Some(allocator.next_counter());
        }
        changed
    }
}

fn preserves_sequential_seed(command: &EditorCommand) -> bool {
    use EditorCommand as C;
    matches!(
        command,
        C::InsertNode { .. }
            | C::BatchInsert { .. }
            | C::CopyNode { .. }
            | C::DuplicateSelected { .. }
            | C::GroupSelected
            | C::PasteClipboard { .. }
            | C::InstantiateComponent { .. }
            | C::UpdateNode { .. }
            | C::MoveNode { .. }
            | C::SetActivePage { .. }
            | C::SetSelection { .. }
            | C::SetSelectionSet { .. }
            | C::ClearSelection
            | C::ToggleNodeSelection { .. }
            | C::SetViewport { .. }
            | C::CopySelected
            | C::SetNodeRotation { .. }
            | C::SetNodeText { .. }
            | C::SetNodeName { .. }
            | C::SetImageVideoSrc { .. }
            | C::SetImageVideoPlayback { .. }
            | C::AddImageVideo { .. }
            | C::RemoveImageVideo { .. }
            | C::SetNodeFontSize { .. }
            | C::SetNodeFontWeight { .. }
            | C::SetNodeFillHex { .. }
            | C::SetNodeStrokeHex { .. }
            | C::SetNodeStrokeWidth { .. }
            | C::SetNodeStrokeSideWidth { .. }
            | C::SetNodeCornerRadius { .. }
            | C::SetNodeFlag { .. }
            | C::SetNodeFlip { .. }
            | C::NudgeSelected { .. }
            | C::AlignSelected { .. }
            | C::SetActiveTool { .. }
    )
}

impl std::ops::Deref for CommandSimulation {
    type Target = EditorState;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

#[path = "atomic_command_allocator.rs"]
mod atomic_allocator;
pub(crate) use atomic_allocator::AtomicCommandAllocator;

#[path = "atomic_command_simulation.rs"]
mod atomic;
pub use atomic::AtomicCommandSimulation;

#[cfg(test)]
#[path = "command_id_scope_tests.rs"]
mod tests;

#[path = "command_id_sequence.rs"]
pub(crate) mod sequence;
pub use sequence::{CommandIdSequence, NodeRead, NodeReadOrder, TrustedNodeLookup};
