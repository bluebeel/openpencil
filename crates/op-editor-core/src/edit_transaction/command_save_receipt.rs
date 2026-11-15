//! Immediate completed receipts only; public mutation/history needs a full save.
use super::*;
impl CommandEditCapture {
    /// Immediate completed state deltas from the actual canonical inverses.
    /// This requires the same exclusive authority with no intervening command
    /// or public mutation. False discards ALL partial visitor output.
    pub fn visit_canonical_deltas<'a>(
        &'a self,
        state: &'a EditorState,
        field_limit: usize,
        mut nodes: impl FnMut(&jian_ops_schema::node::PenNode, &jian_ops_schema::node::PenNode),
        mut headers: impl FnMut(crate::CanonicalHeaderChange) -> bool,
        mut splices: impl FnMut(crate::CanonicalSplice<'a>) -> bool,
    ) -> bool {
        match &self.kind {
            CaptureKind::Scoped(captured)
                if captured.targeted && captured.generation == state.document_generation() =>
            {
                captured.completed.as_ref().is_some_and(|snapshot| {
                    snapshot.visit_canonical_deltas(
                        state,
                        field_limit,
                        &mut nodes,
                        &mut headers,
                        &mut splices,
                    )
                })
            }
            _ => false,
        }
    }

    /// Canonical metadata/headers plus actual append-only splice receipts.
    /// Consumers must discard every partial fragment on a false result.
    pub fn visit_canonical_edits<'a>(
        &'a self,
        state: &'a EditorState,
        field_limit: usize,
        mut nodes: impl FnMut(&jian_ops_schema::node::PenNode, &jian_ops_schema::node::PenNode),
        mut headers: impl FnMut(crate::CanonicalHeaderChange) -> bool,
        mut appends: impl FnMut(crate::CanonicalAppend<'a>) -> bool,
    ) -> bool {
        match &self.kind {
            CaptureKind::Scoped(captured)
                if captured.targeted && captured.generation == state.document_generation() =>
            {
                captured.completed.as_ref().is_some_and(|snapshot| {
                    snapshot.visit_canonical_edits(
                        state,
                        field_limit,
                        &mut nodes,
                        &mut headers,
                        &mut appends,
                    )
                })
            }
            _ => false,
        }
    }

    /// Exact canonical node metadata plus audited root header fields. A false
    /// return invalidates every partial delta; caller must use the full writer.
    /// Each old/current encoded header value is capped by `field_limit`.
    pub fn visit_canonical_changes(
        &self,
        state: &EditorState,
        field_limit: usize,
        mut nodes: impl FnMut(&jian_ops_schema::node::PenNode, &jian_ops_schema::node::PenNode),
        mut headers: impl FnMut(crate::CanonicalHeaderChange) -> bool,
    ) -> bool {
        match &self.kind {
            CaptureKind::Scoped(captured)
                if captured.targeted && captured.generation == state.document_generation() =>
            {
                captured.completed.as_ref().is_some_and(|snapshot| {
                    snapshot.visit_canonical_changes(state, field_limit, &mut nodes, &mut headers)
                })
            }
            _ => false,
        }
    }
}
