//! Explicit undo boundaries for host-authored commands. Raw `apply` remains
//! available for simulation/replay and operations with an existing boundary.

use crate::command_apply::command_marks_document_dirty;
use crate::{EditorCommand, EditorState, IdAllocError, IdAllocator};

impl EditorCommand {
    /// Whether an accepted command can change canonical document content.
    /// Undo/redo change content without recording a new forward undo step.
    pub fn may_change_document(&self) -> bool {
        command_marks_document_dirty(self) || matches!(self, Self::Undo | Self::Redo)
    }
}

impl EditorState {
    /// Group a document command and its synchronous maintenance into one undo
    /// step. View commands and undo/redo do not capture or collapse history.
    /// A rejected/no-change operation preserves both stacks.
    pub fn with_command_history<T>(
        &mut self,
        command: &EditorCommand,
        operation: impl FnOnce(&mut Self) -> T,
    ) -> T {
        if command_marks_document_dirty(command) {
            self.with_history_group(operation)
        } else {
            operation(self)
        }
    }

    /// Apply one local command with a single undo boundary, including raw node
    /// CRUD commands whose low-level implementations leave history to the host.
    pub fn apply_undoable(&mut self, command: EditorCommand) -> bool {
        self.apply_undoable_then(command, |_| {})
    }

    /// Include synchronous post-apply maintenance in the same undo step.
    /// `maintain` runs only for an accepted command; it must not undo/redo or
    /// replace the document. View-only commands keep their history untouched.
    pub fn apply_undoable_then(
        &mut self,
        command: EditorCommand,
        maintain: impl FnOnce(&mut Self),
    ) -> bool {
        self.apply_undoable_then_anchored(command, maintain, None)
    }

    /// Reuse a pending external-commit checkpoint as the undo sharing anchor.
    /// Complete equality still gates reuse; a large first save does not need
    /// two independently cloned sets of node metadata.
    pub fn apply_undoable_captured_then(
        &mut self,
        command: EditorCommand,
        capture: &crate::LocalEditCapture,
        maintain: impl FnOnce(&mut Self),
    ) -> bool {
        self.apply_undoable_then_anchored(command, maintain, Some(capture.undo_anchor()))
    }

    fn apply_undoable_then_anchored(
        &mut self,
        command: EditorCommand,
        maintain: impl FnOnce(&mut Self),
        anchor: Option<&crate::EditorSnapshot>,
    ) -> bool {
        let records_undo = command_marks_document_dirty(&command);
        let operation = |state: &mut Self, checkpoint: Option<&crate::EditorSnapshot>| {
            let applied = state.apply_with_checkpoint(command, checkpoint);
            if applied {
                maintain(state);
            }
            applied
        };
        if records_undo {
            self.with_history_checkpoint(anchor, operation)
        } else {
            operation(self, None)
        }
    }

    /// The session-allocator counterpart to [`Self::apply_undoable`]. Allocator
    /// failures retain their typed result and never produce an undo entry.
    pub fn apply_undoable_with_allocator(
        &mut self,
        command: EditorCommand,
        allocator: &mut dyn IdAllocator,
    ) -> Result<bool, IdAllocError> {
        if command_marks_document_dirty(&command) {
            self.with_history_checkpoint(None, |state, checkpoint| {
                state.apply_with_allocator_checkpoint(command, allocator, checkpoint)
            })
        } else {
            self.apply_with_allocator(command, allocator)
        }
    }
}

#[cfg(test)]
#[path = "command_undo_tests.rs"]
mod tests;

#[path = "command_simulation.rs"]
mod simulation;
