//! Owner-local live-document accounting from exact completed metadata edits.
//! No payload/JSON buffer, per-agent copy, or revision-only content cache.
use super::{charge_cache::RootCharge, new_ledger};
use crate::mcp_serve::command_apply::PreparedCommandEdit;
use op_editor_core::{history_snapshot::BorrowedNodeMetadata, EditorState};
use std::sync::Weak;

type Position = (u64, u64);
fn position(state: &EditorState) -> Position {
    (state.document_generation(), state.document_revision())
}

pub(super) struct DocumentCharge {
    owner: Weak<()>,
    position: Position,
    charge: RootCharge,
}
impl DocumentCharge {
    pub(super) fn new(state: &EditorState, charge: RootCharge) -> Self {
        Self {
            owner: state.derived_cache_owner(),
            position: position(state),
            charge,
        }
    }
    pub(super) fn adjust(self, state: &EditorState, delta: DocumentDelta) -> Option<RootCharge> {
        if !self.owner.ptr_eq(&state.derived_cache_owner())
            || self.position != delta.start
            || !delta.valid
            || delta.end != position(state)
        {
            return None;
        }
        Some(RootCharge {
            serialized: adjust(self.charge.serialized, delta.serialized)?,
            nodes: self.charge.nodes,
            shared_bytes: adjust(self.charge.shared_bytes, delta.boxed)?,
        })
    }
}
fn adjust(old: usize, delta: i128) -> Option<usize> {
    usize::try_from((old as i128).checked_add(delta)?).ok()
}

/// Issued inside one exclusively owned routed dispatch, not arbitrary callbacks.
/// Every changed command must contribute its completed target capture. Anything
/// broader invalidates the receipt and uses a full accounting pass after save.
pub(crate) struct DocumentDelta {
    start: Position,
    end: Position,
    serialized: i128,
    boxed: i128,
    valid: bool,
}
impl DocumentDelta {
    pub(crate) fn begin(state: &EditorState) -> Self {
        Self {
            start: position(state),
            end: position(state),
            serialized: 0,
            boxed: 0,
            valid: true,
        }
    }
    pub(crate) fn observe(
        &mut self,
        before: Position,
        state: &EditorState,
        capture: Option<&PreparedCommandEdit>,
        limit: usize,
    ) {
        let after = position(state);
        if !self.valid || self.end != before || before.0 != after.0 {
            self.valid = false;
            return;
        }
        self.end = after;
        if before == after {
            return;
        }
        let Some(capture) = capture else {
            self.valid = false;
            return;
        };
        let mut failed = false;
        let captured = capture.visit_metadata_changes(state, |old, current| {
            let mut old_count = new_ledger(limit);
            let mut new_count = new_ledger(limit);
            // The same schema serializer/float formatter as the full ledger.
            // Child placeholders have equal presence/bytes and cancel exactly.
            let measured = jian_ops_schema::image_table::without_save_scope(|| {
                old_count.serialize(&BorrowedNodeMetadata::new(old)).is_ok()
                    && new_count
                        .serialize(&BorrowedNodeMetadata::new(current))
                        .is_ok()
            });
            if !measured {
                failed = true;
                return;
            }
            let serialized =
                new_count.serialized.bytes as i128 - old_count.serialized.bytes as i128;
            let boxed = (current.boxed_optional_storage_bytes() as i128
                - old.boxed_optional_storage_bytes() as i128)
                * 2;
            match (
                self.serialized.checked_add(serialized),
                self.boxed.checked_add(boxed),
            ) {
                (Some(a), Some(b)) => {
                    self.serialized = a;
                    self.boxed = b;
                }
                _ => failed = true,
            }
        });
        self.valid &= captured && !failed;
    }
    pub(crate) fn finish(self, state: &EditorState) -> Option<Self> {
        (self.valid && self.end == position(state)).then_some(self)
    }
}
