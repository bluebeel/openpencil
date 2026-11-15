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
        self.chain.invalidate();
        &mut self.state
    }
}
impl TrackedEditor {
    /// Extract only while one writer authority remains exclusively locked.
    /// All document-changing steps must feed receipts or invalidate this chain.
    pub(crate) fn take_save_chain(&mut self) -> SaveChain {
        std::mem::take(&mut self.chain)
    }
    pub(crate) fn restore_save_chain(&mut self, chain: SaveChain) {
        if chain.matches(&self.state) {
            self.chain = chain;
        } else {
            super::hit(super::Event::RestoreMismatch);
            self.chain.invalidate();
        }
    }
}
