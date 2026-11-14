use super::*;
use crate::kiwi::{decode_import_message, decode_message, Definition, Field};

fn field(name: &str, kind: &str, array: bool, value: u32) -> Field {
    Field {
        name: name.into(),
        type_name: kind.into(),
        is_array: array,
        value,
    }
}

fn schema(reverse: bool) -> Schema {
    let mut fields = vec![
        field("sessionID", "uint", false, 1),
        field("localID", "uint", false, 2),
    ];
    if reverse {
        fields.reverse();
    }
    Schema {
        definitions: vec![
            Definition {
                name: "GUID".into(),
                kind: DefKind::Struct,
                fields,
            },
            Definition {
                name: "NodeChange".into(),
                kind: DefKind::Message,
                fields: vec![field("guid", "GUID", false, 1)],
            },
            Definition {
                name: "Message".into(),
                kind: DefKind::Message,
                fields: vec![field("nodeChanges", "NodeChange", true, 1)],
            },
        ],
    }
}

fn wire(reverse: bool, session: u32, local: u32) -> Vec<u8> {
    let mut bytes = vec![1, 1, 1];
    for mut value in if reverse {
        [local, session]
    } else {
        [session, local]
    } {
        loop {
            let next = (value & 127) as u8;
            value >>= 7;
            bytes.push(next | if value == 0 { 0 } else { 128 });
            if value == 0 {
                break;
            }
        }
    }
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

fn guid(root: &FigValue) -> &FigValue {
    root.get_array("nodeChanges").unwrap()[0]
        .get("guid")
        .unwrap()
}

#[test]
fn compact_guid_import_preserves_both_field_orders_and_full_decoder_objects() {
    for reverse in [false, true] {
        let schema = schema(reverse);
        let bytes = wire(reverse, u32::MAX, 0);
        let full = decode_message(&schema, &bytes).unwrap();
        let imported = decode_import_message(&schema, &bytes).unwrap();
        assert!(matches!(guid(&full), FigValue::Object(_)));
        assert!(matches!(guid(&imported), FigValue::Guid(u32::MAX, 0)));
        assert!(guid(&imported).is_object());
        assert_eq!(
            guid(&imported).get_f64("sessionID"),
            Some(f64::from(u32::MAX))
        );
        assert_eq!(guid(&imported).get_f64("localID"), Some(0.0));
        assert_eq!(full, imported, "normalized representation preserves values");
    }
}

#[test]
fn compact_guid_requires_exact_struct_name_field_names_and_u32_types() {
    for change in 0..5 {
        let mut schema = schema(false);
        match change {
            0 => schema.definitions[0].name = "Other".into(),
            1 => schema.definitions[0].fields[0].name = "session".into(),
            2 => schema.definitions[0].fields[0].type_name = "uint64".into(),
            3 => schema.definitions[0].fields[0].is_array = true,
            _ => schema.definitions[0]
                .fields
                .push(field("extension", "uint", false, 3)),
        }
        assert!(definitions(&schema)[0].is_none());
        if change == 2 {
            let imported = decode_import_message(&schema, &wire(false, 1, 2)).unwrap();
            assert!(matches!(guid(&imported), FigValue::Object(_)));
        }
    }
}

#[test]
fn compact_guid_mutation_materializes_without_losing_either_coordinate() {
    let mut value = FigValue::Guid(1, 2);
    *value.get_mut("localID").unwrap() = FigValue::Uint(42);
    assert!(matches!(value, FigValue::Object(_)));
    assert_eq!(value.get_f64("sessionID"), Some(1.0));
    assert_eq!(value.get_f64("localID"), Some(42.0));
    let mut value = FigValue::Guid(1, 2);
    value.set("extension", FigValue::Str("retained".into()));
    assert_eq!(value.get_str("extension"), Some("retained"));
    assert_eq!(value.get_f64("localID"), Some(2.0));
}

#[test]
fn compact_guid_equality_is_transitive_across_generic_and_compact_values() {
    let compact = FigValue::Guid(1, 2);
    let ordinary = FigValue::Object(
        vec![
            ("sessionID".into(), FigValue::Uint(1)),
            ("localID".into(), FigValue::Uint(2)),
        ]
        .into(),
    );
    let reversed = FigValue::Object(
        vec![
            ("localID".into(), FigValue::Uint(2)),
            ("sessionID".into(), FigValue::Uint(1)),
        ]
        .into(),
    );
    assert_eq!(compact, ordinary);
    assert_eq!(ordinary, reversed);
    assert_eq!(compact, reversed);
    assert_ne!(compact, FigValue::Guid(2, 1));
    let other = FigValue::Object(
        vec![
            ("x".into(), FigValue::Uint(1)),
            ("y".into(), FigValue::Uint(2)),
        ]
        .into(),
    );
    let other_reversed = FigValue::Object(
        vec![
            ("y".into(), FigValue::Uint(2)),
            ("x".into(), FigValue::Uint(1)),
        ]
        .into(),
    );
    assert_ne!(
        other, other_reversed,
        "non-GUID ordered-object semantics stay unchanged"
    );
}

#[test]
fn compact_guid_retains_wire_validation_and_depth_ceiling() {
    let schema = schema(false);
    let mut truncated = wire(false, 1, 2);
    truncated.truncate(4);
    assert!(decode_import_message(&schema, &truncated).is_err());
    let mut trailing = wire(false, 1, 2);
    trailing.push(0);
    assert!(decode_import_message(&schema, &trailing).is_err());
    let bytes = [1, 2];
    for scope in [DecodeScope::Full, DecodeScope::Import] {
        let mut decoder = Decoder::new(&schema, &bytes).unwrap();
        assert!(matches!(
            decoder.decode_struct(0, MAX_DEPTH, scope),
            Err(KiwiError::TooDeep)
        ));
    }
}

#[test]
fn compact_guid_does_not_expand_the_dynamic_value_enum() {
    #[cfg(target_pointer_width = "64")]
    assert_eq!(std::mem::size_of::<FigValue>(), 24);
}
