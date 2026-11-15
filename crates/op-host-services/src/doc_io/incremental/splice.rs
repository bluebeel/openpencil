//! Typed metadata serialization retains the schema's declared field order and
//! f64 bits. Only the unchanged child value is replaced by its raw byte span.
use super::index::{fields, Span};
use jian_ops_schema::node::PenNode;
use op_editor_core::{history_snapshot::BorrowedNodeMetadata, PenNodeExt};
use serde::Serialize;
use std::io::{self, Write};
const MAX: usize = 256 * 1024;
struct Limited {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Limited {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(b.len())
            .is_none_or(|n| n > self.limit)
        {
            return Err(io::ErrorKind::OutOfMemory.into());
        }
        self.bytes.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(super) fn encode(value: &impl Serialize) -> Option<Vec<u8>> {
    encode_with_limit(value, MAX)
}
/// Editor catalogs are independent of node headers: full Bridge metadata is
/// larger than 1 MiB. Retained replacements still share the existing EDITS cap.
pub(super) fn editor_meta(value: &impl Serialize) -> Option<Vec<u8>> {
    encode_with_limit(value, 4 * 1024 * 1024)
}
fn encode_with_limit(value: &impl Serialize, limit: usize) -> Option<Vec<u8>> {
    let mut out = Limited {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut out, value).ok()?;
    Some(out.bytes)
}
pub(super) fn append(nodes: &[PenNode]) -> Option<Vec<u8>> {
    let mut resources = false;
    jian_ops_schema::image_table::visit_legacy_node_roots(nodes.iter(), &mut |_| resources = true);
    if resources {
        return None;
    }
    let bytes = jian_ops_schema::image_table::without_save_scope(|| {
        encode_with_limit(&nodes, super::HEADER_VALUE_LIMIT)
    })?;
    (bytes.capacity() <= super::HEADER_VALUE_LIMIT && resource_neutral(&bytes)).then_some(bytes)
}
fn resource_neutral(bytes: &[u8]) -> bool {
    [
        b"\"url\":".as_slice(),
        b"\"src\":".as_slice(),
        b"\"thumbnailUrl\":".as_slice(),
    ]
    .iter()
    .all(|key| !bytes.windows(key.len()).any(|part| part == *key))
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
/// One isolated collector assigns collision-safe short identities across BOTH
/// headers. Exact field equality therefore compares original ImageSrc contents
/// without escaping/copying their large payloads. Its tables are never emitted.
pub(super) fn resource_metadata_pair(
    old: &PenNode,
    current: &PenNode,
    thumbnails: &jian_ops_schema::image_thumbs::ImageThumbSnapshot,
) -> Option<(Vec<u8>, Vec<u8>)> {
    // Compare actual typed sources as well as the compact representation:
    // unresolved op-image: text must not alias a different real source, and
    // equal hash text alone is never proof. Arc clones do not copy payloads.
    let mut sources = Vec::new();
    let mut valid = true;
    jian_ops_schema::image_table::visit_node_image_sources(old, &mut |source| {
        if sources.len() == 4096 {
            valid = false;
        } else {
            sources.push(source.clone());
        }
    });
    if !valid {
        return None;
    }
    let mut at = 0;
    jian_ops_schema::image_table::visit_node_image_sources(current, &mut |source| {
        if sources.get(at) != Some(source) {
            valid = false;
        }
        if at < 4096 {
            at += 1;
        } else {
            valid = false;
        }
    });
    if !valid || at != sources.len() {
        return None;
    }
    jian_ops_schema::image_table::with_save_scope(thumbnails, |_| {
        let before = if current.children().is_some() && old.children().is_none() {
            // Completed metadata snapshots have no descendants. Restore only
            // their separately certified child-array presence, as old_metadata.
            let mut header = old.clone();
            header.children_mut()?;
            encode(&BorrowedNodeMetadata::new(&header))?
        } else if current.children().is_some() == old.children().is_some() {
            encode(&BorrowedNodeMetadata::new(old))?
        } else {
            return None;
        };
        let after = encode(&BorrowedNodeMetadata::new(current))?;
        Some((before, after))
    })
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
