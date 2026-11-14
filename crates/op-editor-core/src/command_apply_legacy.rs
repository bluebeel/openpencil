//! Standalone compatibility wrapper for command application.

use crate::{DocumentIdAllocator, EditorCommand, EditorState};

#[path = "command_allocation_policy.rs"]
mod allocation_policy;

/// Private to one top-level InsertNode/BatchInsert. Its initial counter is
/// above every current node/page id, and these commands introduce only ids
/// minted through this same allocator. Mixed batches keep full collision sets.
struct LeafHighWaterAllocator(DocumentIdAllocator);

impl crate::IdAllocator for LeafHighWaterAllocator {
    fn document_collision_ids(
        &self,
        _document: &jian_ops_schema::PenDocument,
    ) -> std::collections::HashSet<crate::NodeId> {
        std::collections::HashSet::new()
    }

    fn allocate(
        &mut self,
        taken: &mut std::collections::HashSet<crate::NodeId>,
    ) -> Result<crate::NodeId, crate::IdAllocError> {
        crate::IdAllocator::allocate(&mut self.0, taken)
    }
}

#[cfg(test)]
#[path = "command_allocation_tests.rs"]
mod allocation_tests;

impl EditorState {
    /// Apply one command with the historical sequential document id policy.
    ///
    /// Returns `false` on validation or id exhaustion. Collaboration hosts
    /// must call [`Self::apply_with_allocator`] with their session allocator.
    pub fn apply(&mut self, command: EditorCommand) -> bool {
        self.apply_with_checkpoint(command, None)
    }

    /// Apply with the caller's document-wide ID policy. Validation returns
    /// Ok(false); exhausted allocation retains its typed error.
    pub fn apply_with_allocator(
        &mut self,
        command: EditorCommand,
        allocator: &mut dyn crate::IdAllocator,
    ) -> Result<bool, crate::IdAllocError> {
        self.apply_with_allocator_checkpoint(command, allocator, None)
    }

    pub(crate) fn apply_with_checkpoint(
        &mut self,
        command: EditorCommand,
        checkpoint: Option<&crate::EditorSnapshot>,
    ) -> bool {
        let mut allocator = if allocation_policy::requires_id_seed(&command) {
            let Ok(allocator) = DocumentIdAllocator::sequential_for_document(&self.doc) else {
                return false;
            };
            allocator
        } else {
            DocumentIdAllocator::sequential(1)
        };
        if matches!(
            command,
            EditorCommand::InsertNode { .. } | EditorCommand::BatchInsert { .. }
        ) {
            self.apply_with_allocator_checkpoint(
                command,
                &mut LeafHighWaterAllocator(allocator),
                checkpoint,
            )
            .unwrap_or(false)
        } else {
            self.apply_with_allocator_checkpoint(command, &mut allocator, checkpoint)
                .unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_support::{rect, state_with},
        NodeId,
    };

    #[test]
    fn view_commands_work_without_available_document_ids() {
        let id = NodeId::new(format!("n{}", u64::MAX));
        let mut state = state_with(vec![rect(id.as_str(), "Existing", 0.0, 0.0, 8.0, 8.0)]);
        let document = state.doc.clone();
        let revision = state.document_revision();
        assert!(state.apply(EditorCommand::SetViewport {
            pan_x: Some(20),
            pan_y: Some(30),
            zoom_percent: Some(125),
        }));
        assert!(state.apply(EditorCommand::SetSelection {
            node_id: id.clone()
        }));
        assert!(state.apply(EditorCommand::CopySelected));
        assert_eq!(state.clipboard.len(), 1);
        assert_eq!(state.doc, document);
        assert_eq!(state.document_revision(), revision);
        assert_eq!(state.viewport.zoom, 1.25);
        // Allocating commands still preserve the exhaustion contract.
        assert!(!state.apply(EditorCommand::DuplicateSelected { offset_px: 10 }));
        assert_eq!(state.doc, document);
    }
}
