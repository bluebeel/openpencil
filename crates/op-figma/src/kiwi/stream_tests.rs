use super::*;
use std::io::{BufReader, Cursor};

fn field(name: &str, kind: &str, array: bool, id: u32) -> Field {
    Field {
        name: name.into(),
        type_name: kind.into(),
        is_array: array,
        value: id,
    }
}
fn definition(name: &str, kind: DefKind, fields: Vec<Field>) -> Definition {
    Definition {
        name: name.into(),
        kind,
        fields,
    }
}
fn uint(out: &mut Vec<u8>, mut value: u32) {
    while value >= 128 {
        out.push(value as u8 | 128);
        value >>= 7;
    }
    out.push(value as u8);
}
fn streamed(schema: &Schema, bytes: &[u8]) -> Result<FigValue, KiwiError> {
    decode(
        schema,
        BufReader::with_capacity(1, Cursor::new(bytes)),
        bytes.len(),
    )
}
fn indexed_hydrated(schema: &Schema, wire: &[u8]) -> Result<FigValue, KiwiError> {
    let mut decoded = decode_indexed(
        schema,
        BufReader::with_capacity(1, Cursor::new(wire)),
        wire.len(),
    )?;
    if let Some(encoded) = decoded.nodes {
        let nodes = decoded.root.get_array("nodeChanges").unwrap().to_vec();
        decoded
            .root
            .set("nodeChanges", FigValue::Array(encoded.all(nodes).into()));
    }
    Ok(decoded.root)
}

fn fixture() -> (Schema, Vec<u8>) {
    let schema = Schema {
        definitions: vec![
            definition(
                "Future",
                DefKind::Message,
                vec![
                    field("label", "string", false, 1),
                    field("values", "int64", true, 2),
                    field("bytes", "byte", true, 3),
                    field("switch", "bool", false, 4),
                    field("weight", "float", false, 5),
                ],
            ),
            definition(
                "NodeChange",
                DefKind::Message,
                vec![field("futureUnknown", "Future", false, 1)],
            ),
            definition(
                "Message",
                DefKind::Message,
                vec![
                    field("nodeChanges", "NodeChange", true, 1),
                    field("rootUnknown", "uint", false, 2),
                ],
            ),
        ],
    };
    let wire = vec![
        2, 7, 1, 1, 1, 1, b'a', 0xff, b'b', 0, 2, 2, 3, 8, 3, 3, 0, 255, 128, 4, 1, 5, 0, 1, b'z',
        0, 0, 0, 2, 11, 0,
    ];
    (schema, wire)
}

#[test]
fn bounded_stream_preserves_unknown_fields_order_duplicates_and_chunk_boundaries() {
    let (schema, wire) = fixture();
    let eager = decode_import_message(&schema, &wire).unwrap();
    let streamed = streamed(&schema, &wire).unwrap();
    assert_eq!(indexed_hydrated(&schema, &wire).unwrap(), eager);
    assert_eq!(streamed, eager);
    assert_eq!(streamed.get_f64("rootUnknown"), Some(11.0));
    let nested = &streamed.get_array("nodeChanges").unwrap()[0];
    let future = nested.get("futureUnknown").unwrap();
    assert_eq!(future.get_str("label"), Some("z"));
    assert_eq!(
        future.get("bytes").unwrap().as_bytes(),
        Some(&[0, 255, 128][..])
    );
    assert_eq!(
        future.get_array("values").unwrap(),
        &[FigValue::Int64(-2), FigValue::Int64(4)]
    );
    assert!(matches!(&streamed, FigValue::Object(fields) if fields[0].0.as_ref()=="rootUnknown"));
    for end in 0..wire.len() {
        assert!(decode_import_message(&schema, &wire[..end]).is_err());
        assert!(self::streamed(&schema, &wire[..end]).is_err());
        assert!(indexed_hydrated(&schema, &wire[..end]).is_err());
    }
    let mut trailing = wire.clone();
    trailing.push(99);
    assert!(matches!(
        self::streamed(&schema, &trailing),
        Err(KiwiError::TrailingBytes(1))
    ));
    let mut unknown = wire;
    unknown[0] = 127;
    assert!(matches!(
        indexed_hydrated(&schema, &unknown),
        Err(KiwiError::UnknownField(_))
    ));
    assert!(matches!(
        self::streamed(&schema, &unknown),
        Err(KiwiError::UnknownField(_))
    ));
}

#[test]
fn bounded_stream_preserves_zero_byte_array_validation_through_lazy_subranges() {
    let schema = Schema {
        definitions: vec![
            definition("Empty", DefKind::Struct, vec![]),
            definition(
                "Cache",
                DefKind::Message,
                vec![
                    field("payload", "string", false, 1),
                    field("future", "Empty", true, 2),
                ],
            ),
            definition(
                "NodeChange",
                DefKind::Message,
                vec![field("derivedSymbolData", "Cache", true, 1)],
            ),
            definition(
                "Message",
                DefKind::Message,
                vec![
                    field("nodeChanges", "NodeChange", true, 1),
                    field("padding", "byte", true, 2),
                ],
            ),
        ],
    };
    let mut wire = vec![1, 1, 1, 1, 1];
    wire.extend(std::iter::repeat_n(b'X', 300));
    wire.extend([0, 2]);
    uint(&mut wire, 200);
    wire.extend([0, 0, 2]);
    uint(&mut wire, 512);
    wire.extend(std::iter::repeat_n(0, 512));
    wire.push(0);
    let full = decode_message(&schema, &wire).unwrap();
    for decoded in [
        decode_import_message(&schema, &wire).unwrap(),
        streamed(&schema, &wire).unwrap(),
        indexed_hydrated(&schema, &wire).unwrap(),
    ] {
        assert_eq!(
            decoded, full,
            "lazy hydration retains the original logical suffix length"
        );
        let node = &decoded.get_array("nodeChanges").unwrap()[0];
        assert_eq!(
            node.get_array("derivedSymbolData").unwrap()[0]
                .get_array("future")
                .unwrap()
                .len(),
            200
        );
    }
    let invalid = vec![1, 1, 1, 1, 2, 200, 1, 0, 0, 0];
    assert!(decode_import_message(&schema, &invalid).is_err());
    assert!(streamed(&schema, &invalid).is_err());
    assert!(indexed_hydrated(&schema, &invalid).is_err());
}

#[test]
fn bounded_stream_keeps_depth_and_array_length_limits() {
    let schema = Schema {
        definitions: vec![
            definition(
                "Nest",
                DefKind::Message,
                vec![field("child", "Nest", false, 1)],
            ),
            definition(
                "Message",
                DefKind::Message,
                vec![field("nodeChanges", "Nest", true, 1)],
            ),
        ],
    };
    let mut wire = vec![1, 1];
    wire.extend(std::iter::repeat_n(1, MAX_DEPTH as usize + 1));
    wire.extend(std::iter::repeat_n(0, MAX_DEPTH as usize + 3));
    assert!(matches!(
        decode_import_message(&schema, &wire),
        Err(KiwiError::TooDeep)
    ));
    assert!(matches!(streamed(&schema, &wire), Err(KiwiError::TooDeep)));
    assert!(matches!(
        indexed_hydrated(&schema, &wire),
        Err(KiwiError::TooDeep)
    ));
    assert!(matches!(
        streamed(&schema, &[1, 255, 255, 255, 255, 15, 0]),
        Err(KiwiError::OutOfBounds)
    ));
    fn send_sync<T: Send + Sync>() {}
    send_sync::<FigValue>();
    assert_eq!(std::mem::size_of::<FigValue>(), 24);
}

#[test]
fn indexed_frames_follow_repeated_field_slots_not_last_wire_event() {
    let schema = Schema {
        definitions: vec![
            definition(
                "NodeChange",
                DefKind::Message,
                vec![
                    field("name", "string", false, 1),
                    field("payload", "uint", false, 2),
                ],
            ),
            definition(
                "Message",
                DefKind::Message,
                vec![
                    field("nodeChanges", "NodeChange", true, 1),
                    field("nodeChanges", "NodeChange", true, 2),
                ],
            ),
        ],
    };
    // Slot 0 is overwritten after slot 1: extraction still selects slot 1.
    let wire = [
        1, 1, 1, b'a', 0, 2, 10, 0, 2, 1, 1, b'b', 0, 2, 20, 0, 1, 1, 1, b'c', 0, 2, 30, 0, 0,
    ];
    let eager = decode_import_message(&schema, &wire).unwrap();
    let full = crate::figma_types::extract_decoded_root(eager).unwrap().0;
    let decoded = decode_indexed(&schema, Cursor::new(&wire), wire.len()).unwrap();
    let metadata = crate::figma_types::extract_decoded_root(decoded.root)
        .unwrap()
        .0;
    assert!(metadata[0].get("payload").is_none());
    let hydrated = decoded.nodes.unwrap().all(metadata);
    assert_eq!(hydrated, full);
    assert_eq!(hydrated[0].get_str("name"), Some("b"));
    assert_eq!(hydrated[0].get_f64("payload"), Some(20.0));
}
