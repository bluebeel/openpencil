//! Standalone compatibility wrapper for command application.

use crate::{DocumentIdAllocator, EditorCommand, EditorState};

impl EditorState {
    /// Apply one command with the historical sequential document id policy.
    ///
    /// Returns `false` on validation or id exhaustion. Collaboration hosts
    /// must call [`Self::apply_with_allocator`] with their session allocator.
    pub fn apply(&mut self, command: EditorCommand) -> bool {
        // The existing dirty classification excludes only commands that never
        // allocate document ids (viewport, selection, clipboard copy, undo,
        // redo and view-only state). They must not walk every node to seed an
        // allocator they will never call, nor fail because ids are exhausted.
        let mut allocator = if crate::command_apply::command_marks_document_dirty(&command) {
            let Ok(allocator) = DocumentIdAllocator::sequential_for_document(&self.doc) else {
                return false;
            };
            allocator
        } else {
            DocumentIdAllocator::sequential(1)
        };
        self.apply_with_allocator(command, &mut allocator)
            .unwrap_or(false)
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
