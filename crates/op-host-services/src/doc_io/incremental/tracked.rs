//! Public mutable editor access cannot silently inherit a saved-byte proof.
use super::SaveChain;
use op_editor_core::EditorState;
use std::ops::{Deref, DerefMut};
pub(crate) struct TrackedEditor {
    state: EditorState,
    chain: SaveChain,
}
impl From<EditorState> for TrackedEditor {
    fn from(state: EditorState) -> Self {
        Self {
            state,
            chain: SaveChain::default(),
        }
    }
}
impl Deref for TrackedEditor {
    type Target = EditorState;
    fn deref(&self) -> &EditorState {
        &self.state
    }
}
impl DerefMut for TrackedEditor {
    fn deref_mut(&mut self) -> &mut EditorState {
        super::hit(super::Event::MutableInvalidation);
        if self.chain.base.is_some() {
            super::hit(super::Event::PopulatedMutableInvalidation);
        }
        self.chain.invalidate();
        &mut self.state
    }
}
impl TrackedEditor {
    /// Collaboration projection has no canonical document, persisted editor
    /// metadata, history or resource access. The runtime can refresh its panel
    /// and presence without obtaining an untracked mutable EditorState.
    pub(crate) fn collaboration_ui_mut(&mut self) -> &mut op_editor_core::CollabUiState {
        super::hit(super::Event::CollaborationProjection);
        &mut self.state.editor_ui.collab
    }
    /// Extract only while one writer authority remains exclusively locked.
    /// All document-changing steps must feed receipts or invalidate this chain.
    pub(crate) fn take_save_chain(&mut self) -> SaveChain {
        super::hit(if self.chain.base.is_some() {
            super::Event::TakePopulated
        } else {
            super::Event::TakeEmpty
        });
        std::mem::take(&mut self.chain)
    }
    pub(crate) fn restore_save_chain(&mut self, chain: SaveChain) {
        if chain.matches(&self.state) {
            super::hit(super::Event::RestoreReady);
            self.chain = chain;
        } else {
            super::hit(super::Event::RestoreMismatch);
            self.chain.invalidate();
        }
    }
}
