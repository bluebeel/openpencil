use super::*;
use crate::kiwi::{decode_import_message, decode_message, Definition, Field};

fn field(name: &str, kind: &str, array: bool, id: u32) -> Field {
    Field {
        name: name.into(),
        type_name: kind.into(),
        is_array: array,
        value: id,
    }
}
fn schema(kind: FloatKind, reverse: bool) -> Schema {
    let mut fields = kind
        .fields()
        .iter()
        .enumerate()
        .map(|(i, name)| field(name, "float", false, i as u32 + 1))
        .collect::<Vec<_>>();
    if reverse {
        fields.reverse();
    }
    Schema {
        definitions: vec![
            Definition {
                name: "Record".into(),
                kind: DefKind::Struct,
                fields,
            },
            Definition {
                name: "NodeChange".into(),
                kind: DefKind::Message,
                fields: vec![field("record", "Record", false, 1)],
            },
            Definition {
                name: "Message".into(),
                kind: DefKind::Message,
                fields: vec![field("nodeChanges", "NodeChange", true, 1)],
            },
        ],
    }
}
fn scalar_wire(schema: &Schema, kind: FloatKind, values: [f32; 4]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for field in &schema.definitions[0].fields {
        let index = kind
            .fields()
            .iter()
            .position(|name| *name == field.name)
            .unwrap();
        let value = values[index];
        if value == 0.0 {
            bytes.push(0);
        } else {
            bytes.extend_from_slice(&value.to_bits().rotate_left(9).to_le_bytes());
        }
    }
    bytes
}
fn wire(schema: &Schema, kind: FloatKind, values: [f32; 4]) -> Vec<u8> {
    let mut bytes = vec![1, 1, 1];
    bytes.extend(scalar_wire(schema, kind, values));
    bytes.extend([0, 0]);
    bytes
}
fn record(root: &FigValue) -> &FigValue {
    root.get_array("nodeChanges").unwrap()[0]
        .get("record")
        .unwrap()
}
fn ordinary(kind: FloatKind, values: [f32; 4], reverse: bool) -> FigValue {
    let mut fields = kind
        .fields()
        .iter()
        .zip(values)
        .map(|(name, value)| ((*name).into(), FigValue::Float(value)))
        .collect::<Vec<_>>();
    if reverse {
        fields.reverse();
    }
    FigValue::Object(fields)
}

#[test]
fn packed_vectors_and_colors_preserve_full_decoding_lookup_and_schema_order() {
    assert_eq!(std::mem::size_of::<FigValue>(), 32);
    for kind in [FloatKind::Vector, FloatKind::Color] {
        for reverse in [false, true] {
            let schema = schema(kind, reverse);
            let bytes = wire(&schema, kind, [1.25, -2.5, 0.125, 0.75]);
            let full = decode_message(&schema, &bytes).unwrap();
            let packed = decode_import_message(&schema, &bytes).unwrap();
            assert!(matches!(record(&full), FigValue::Object(_)));
            assert!(matches!(
                record(&packed),
                FigValue::Vec2(_) | FigValue::Color(_)
            ));
            assert_eq!(full, packed);
            assert!(record(&packed).is_object());
            for name in kind.fields() {
                assert_eq!(record(&full).get_f64(name), record(&packed).get_f64(name));
                assert_eq!(record(&full).get(name), record(&packed).get(name));
            }
            assert!(record(&packed).get("extension").is_none());
        }
    }
}

#[test]
fn packed_float_schema_recognition_rejects_extensions_duplicates_and_other_types() {
    for kind in [FloatKind::Vector, FloatKind::Color] {
        for change in 0..5 {
            let mut schema = schema(kind, false);
            match change {
                0 => schema.definitions[0].kind = DefKind::Message,
                1 => schema.definitions[0].fields[0].type_name = "uint".into(),
                2 => schema.definitions[0].fields[0].is_array = true,
                3 => {
                    schema.definitions[0].fields[0].name =
                        schema.definitions[0].fields[1].name.clone()
                }
                _ => schema.definitions[0]
                    .fields
                    .push(field("extension", "float", false, 7)),
            }
            assert!(definitions(&schema)[0].is_none());
        }
    }
}

#[test]
fn scalar_views_are_lazy_and_invalidated_by_inline_mutation() {
    let mut record = FloatRecord::new([1.25, -2.5]);
    assert_eq!(record.retained_bytes(), 0);
    assert_eq!(record.get("x", &["x", "y"]), Some(&FigValue::Float(1.25)));
    assert_eq!(record.retained_bytes(), 2 * std::mem::size_of::<FigValue>());
    assert!(record.set("x", &FigValue::Float(-0.0), &["x", "y"]));
    assert_eq!(record.retained_bytes(), 0);
    assert_eq!(
        record
            .get("x", &["x", "y"])
            .unwrap()
            .as_f64()
            .unwrap()
            .to_bits(),
        (-0.0f64).to_bits()
    );
    let value = FigValue::Vec2(record);
    let _ = value.get_f64("x");
    let FigValue::Vec2(record) = &value else {
        unreachable!()
    };
    // The preceding borrowed read remains valid in a clone; numeric lookup
    // never allocates another scalar read view.
    assert_eq!(value.clone(), value);
    assert_eq!(record.retained_bytes(), 2 * std::mem::size_of::<FigValue>());
    let numeric_only = FigValue::Color(FloatRecord::new([0.1, 0.2, 0.3, 0.4]));
    assert!(numeric_only.get_f64("a").is_some());
    let FigValue::Color(record) = numeric_only else {
        unreachable!()
    };
    assert_eq!(record.retained_bytes(), 0);
}

#[test]
fn packed_float_mutations_preserve_all_fields_and_materialize_incompatible_changes() {
    for kind in [FloatKind::Vector, FloatKind::Color] {
        let values = [1.25, -2.5, 0.125, 0.75];
        let mut value = kind.value(values);
        value.set(kind.fields()[0], FigValue::Float(-0.0));
        assert!(matches!(value, FigValue::Vec2(_) | FigValue::Color(_)));
        assert_eq!(
            value.get_f64(kind.fields()[0]).unwrap().to_bits(),
            (-0.0f64).to_bits()
        );
        *value.get_mut(kind.fields()[1]).unwrap() = FigValue::Float(5.0);
        assert!(matches!(value, FigValue::Object(_)));
        assert_eq!(
            value.get_f64(kind.fields()[0]).unwrap().to_bits(),
            (-0.0f64).to_bits()
        );
        let mut value = kind.value(values);
        value.set("extension", FigValue::Str("retained".into()));
        assert_eq!(value.get_str("extension"), Some("retained"));
        assert_eq!(value.get_f64(kind.fields()[1]), Some(-2.5));
        let mut value = kind.value(values);
        value.set(kind.fields()[0], FigValue::Uint(9));
        assert!(matches!(value, FigValue::Object(_)));
        assert_eq!(value.get_f64(kind.fields()[0]), Some(9.0));
    }
}

#[test]
fn packed_float_logical_equality_is_normalized_and_preserves_ieee_semantics() {
    for kind in [FloatKind::Vector, FloatKind::Color] {
        let values = [1.25, -2.5, 0.125, 0.75];
        let packed = kind.value(values);
        let raw = ordinary(kind, values, false);
        let reordered = ordinary(kind, values, true);
        assert_eq!(packed, raw);
        assert_eq!(raw, reordered);
        assert_eq!(packed, reordered);
        let mut minus = [0.0; 4];
        minus[0] = -0.0;
        assert_eq!(kind.value([0.0; 4]), kind.value(minus));
        let mut nan = values;
        nan[0] = f32::from_bits(0x7fc01234);
        assert_ne!(kind.value(nan), kind.value(nan));
        assert_ne!(kind.value(nan), ordinary(kind, nan, false));
    }
}

#[test]
fn packed_float_wire_depth_projection_and_special_values_match_generic_contracts() {
    for kind in [FloatKind::Vector, FloatKind::Color] {
        let schema = schema(kind, false);
        let values = [
            f32::from_bits(0x7fc01234),
            f32::INFINITY,
            f32::NEG_INFINITY,
            1.0,
        ];
        let bytes = wire(&schema, kind, values);
        let full = decode_message(&schema, &bytes).unwrap();
        let packed = decode_import_message(&schema, &bytes).unwrap();
        for name in kind.fields() {
            assert_eq!(
                record(&full).get(name).unwrap().as_f64().unwrap().to_bits(),
                record(&packed)
                    .get(name)
                    .unwrap()
                    .as_f64()
                    .unwrap()
                    .to_bits()
            );
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode_import_message(&schema, &trailing).is_err());
        assert!(decode_import_message(&schema, &bytes[..5]).is_err());
        let raw = scalar_wire(&schema, kind, values);
        for scope in [DecodeScope::Full, DecodeScope::Import] {
            let mut decoder = Decoder::new(&schema, &raw).unwrap();
            assert!(matches!(
                decoder.decode_struct(0, MAX_DEPTH, scope),
                Err(KiwiError::TooDeep)
            ));
        }
        let mut decoder = Decoder::new(&schema, &raw).unwrap();
        let skipped = decoder.decode_struct(0, 0, DecodeScope::Presence).unwrap();
        assert!(matches!(skipped, FigValue::Object(fields) if fields.is_empty()));
    }
}

#[test]
fn packed_float_views_preserve_send_sync_and_concurrent_shared_reads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<FigValue>();
    let value = FigValue::Color(FloatRecord::new([0.25, 0.5, 0.75, 1.0]));
    let barrier = std::sync::Barrier::new(16);
    std::thread::scope(|scope| {
        for _ in 0..16 {
            let (value, barrier) = (&value, &barrier);
            scope.spawn(move || {
                barrier.wait();
                for _ in 0..100 {
                    assert_eq!(value.get("a"), Some(&FigValue::Float(1.0)));
                    assert_eq!(value.get("r"), Some(&FigValue::Float(0.25)));
                }
            });
        }
    });
    let FigValue::Color(record) = &value else {
        unreachable!()
    };
    assert_eq!(record.retained_bytes(), 4 * std::mem::size_of::<FigValue>());
    let clone = value.clone();
    assert_eq!(clone, value);
    let FigValue::Color(clone) = clone else {
        unreachable!()
    };
    assert_eq!(
        clone.retained_bytes(),
        0,
        "read views are disposable clone caches"
    );
}
