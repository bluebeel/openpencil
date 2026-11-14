//! Explicit undo payloads: complete legacy snapshots or trusted scoped swaps.
use super::{EditorSnapshot, ScopedEditorSnapshot};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub enum HistoryEntry {
    Full(EditorSnapshot),
    Scoped(Arc<ScopedEditorSnapshot>),
}
impl From<EditorSnapshot> for HistoryEntry {
    fn from(value: EditorSnapshot) -> Self {
        Self::Full(value)
    }
}
impl HistoryEntry {
    pub fn full_snapshot(&self) -> Option<&EditorSnapshot> {
        match self {
            Self::Full(snapshot) => Some(snapshot),
            Self::Scoped(_) => None,
        }
    }
    pub fn full_snapshot_mut(&mut self) -> Option<&mut EditorSnapshot> {
        match self {
            Self::Full(snapshot) => Some(snapshot),
            Self::Scoped(_) => None,
        }
    }
    pub fn scoped_snapshot(&self) -> Option<&Arc<ScopedEditorSnapshot>> {
        match self {
            Self::Scoped(snapshot) => Some(snapshot),
            Self::Full(_) => None,
        }
    }
    pub fn revision(&self) -> u64 {
        match self {
            Self::Full(snapshot) => snapshot.revision,
            Self::Scoped(snapshot) => snapshot.revision,
        }
    }
    pub fn active_page_index(&self) -> usize {
        match self {
            Self::Full(snapshot) => snapshot.active_page_index,
            Self::Scoped(snapshot) => snapshot.active_page_index,
        }
    }
}
