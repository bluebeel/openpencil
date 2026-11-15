//! Typed metadata serialization retains the schema's declared field order and
//! f64 bits. Only the unchanged child value is replaced by its raw byte span.
use super::index::{fields, Span};
use jian_ops_schema::node::PenNode;
use op_editor_core::{history_snapshot::BorrowedNodeMetadata, PenNodeExt};
use serde::Serialize;
use std::io::{self, Write};
const MAX: usize = 256 * 1024;
struct Limited(Vec<u8>);
impl Write for Limited {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.0.len().checked_add(b.len()).is_none_or(|n| n > MAX) {
            return Err(io::ErrorKind::OutOfMemory.into());
        }
        self.0.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(super) fn encode(value: &impl Serialize) -> Option<Vec<u8>> {
    let mut out = Limited(Vec::new());
    serde_json::to_writer(&mut out, value).ok()?;
    Some(out.0)
}
pub(super) fn metadata(node: &PenNode) -> Option<Vec<u8>> {
    let bytes = encode(&BorrowedNodeMetadata::new(node))?;
    // Resource-bearing metadata remains on the complete collector writer.
    // This is a conservative eligibility refusal, never field projection.
    for key in [
        b"\"url\":".as_slice(),
        b"\"src\":".as_slice(),
        b"\"thumbnailUrl\":".as_slice(),
    ] {
        if bytes.windows(key.len()).any(|part| part == key) {
            return None;
        }
    }
    // A metadata receipt guarantees unchanged child identity/presence; this
    // serializer does not inspect or clone the actual descendant forest.
    let _ = node.children();
    Some(bytes)
}
pub(super) fn parts(bytes: &[u8]) -> Option<Option<Span>> {
    Some(
        fields(
            bytes,
            Span {
                start: 0,
                end: bytes.len(),
            },
        )?
        .into_iter()
        .find_map(|(key, value)| (key == "children").then_some(value)),
    )
}

/// A SharedNode header has children=None even when its separately retained
/// wire witness is Some(empty). Reconstitute that exact proven presence using
/// an empty placeholder, never the actual descendant tree or geometry kind.
pub(super) fn old_metadata(node: &PenNode, children_present: bool) -> Option<Vec<u8>> {
    let bytes = metadata(node)?; // cap the original metadata before cloning it
    if children_present && node.children().is_none() {
        let mut header = node.clone(); // supplied old metadata has no children
        header.children_mut()?;
        metadata(&header)
    } else if children_present == node.children().is_some() {
        Some(bytes)
    } else {
        None
    }
}
