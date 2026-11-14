use super::*;
use crate::kiwi::{decode_import_message, DefKind, Definition, Field, Schema};
use std::io::{BufReader, Cursor};
fn field(name: &str, kind: &str, array: bool, id: u32) -> Field {
    Field {
        name: name.into(),
        type_name: kind.into(),
        is_array: array,
        value: id,
    }
}
fn uint(out: &mut Vec<u8>, mut n: u32) {
    while n >= 128 {
        out.push(n as u8 | 128);
        n >>= 7
    }
    out.push(n as u8)
}
fn append_blob(wire: &mut Vec<u8>, field: u8, byte: u8) {
    wire.extend([field, 1, 1]);
    uint(wire, 4096);
    wire.extend(std::iter::repeat_n(byte, 4096));
    wire.extend([2, b'f', b'u', b't', b'u', b'r', b'e', 0, 0]);
}
#[test]
fn streamed_blob_extraction_preserves_unknown_validation_and_duplicate_slot_precedence() {
    let schema = Schema {
        definitions: vec![
            Definition {
                name: "Blob".into(),
                kind: DefKind::Message,
                fields: vec![
                    field("bytes", "byte", true, 1),
                    field("futureUnknown", "string", false, 2),
                ],
            },
            Definition {
                name: "Node".into(),
                kind: DefKind::Message,
                fields: vec![
                    field("guid", "uint", false, 1),
                    field("futureUnknown", "uint", false, 2),
                ],
            },
            Definition {
                name: "Message".into(),
                kind: DefKind::Message,
                fields: vec![
                    field("nodeChanges", "Node", true, 1),
                    field("blobs", "Blob", true, 2),
                    field("blobs", "Blob", true, 3),
                ],
            },
        ],
    };
    let mut wire = vec![1, 1, 1, 7, 2, 91, 0];
    append_blob(&mut wire, 2, b'A');
    append_blob(&mut wire, 3, b'B');
    append_blob(&mut wire, 2, b'C');
    wire.push(0);
    let full = decode_import_message(&schema, &wire).unwrap();
    let (original_nodes, original_blobs) = extract_decoded_root(full.clone()).unwrap();
    assert_eq!(original_blobs[0].as_bytes().unwrap(), vec![b'B'; 4096]);
    let (raw, packed) = crate::kiwi::stream::decode_with_blobs(
        &schema,
        BufReader::with_capacity(3, Cursor::new(&wire)),
        wire.len(),
        true,
    )
    .unwrap();
    let (nodes, _) = extract_decoded_root(raw).unwrap();
    let packed = packed.unwrap();
    assert_eq!(nodes, original_nodes);
    assert_eq!(nodes[0].get_f64("futureUnknown"), Some(91.0));
    assert!(matches!(&packed[0], BlobOrString::Packed(_)));
    assert_eq!(
        packed, original_blobs,
        "final insertion-order slot wins, even when another field was repeated later on the wire"
    );
    let (eager_raw, none) =
        crate::kiwi::stream::decode_with_blobs(&schema, Cursor::new(&wire), wire.len(), false)
            .unwrap();
    assert!(none.is_none());
    assert_eq!(eager_raw, full);
    let mut invalid = wire.clone();
    let end = invalid.len();
    invalid[end - 2] = 127;
    assert!(crate::kiwi::stream::decode_with_blobs(
        &schema,
        Cursor::new(&invalid),
        invalid.len(),
        true
    )
    .is_err());
    assert!(decode_import_message(&schema, &invalid).is_err());
}
