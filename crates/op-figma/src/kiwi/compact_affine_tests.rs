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
fn schema(order: &[usize]) -> Schema {
    Schema {
        definitions: vec![
            Definition {
                name: "Transform".into(),
                kind: DefKind::Struct,
                fields: order
                    .iter()
                    .enumerate()
                    .map(|(i, j)| field(FIELDS[*j], "float", false, i as u32 + 1))
                    .collect(),
            },
            Definition {
                name: "NodeChange".into(),
                kind: DefKind::Message,
                fields: vec![field("transform", "Transform", false, 1)],
            },
            Definition {
                name: "Message".into(),
                kind: DefKind::Message,
                fields: vec![field("nodeChanges", "NodeChange", true, 1)],
            },
        ],
    }
}
fn float_bytes(order: &[usize], values: [f32; 6]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for index in order {
        let value = values[*index];
        if value == 0.0 {
            bytes.push(0);
        } else {
            bytes.extend_from_slice(&value.to_bits().rotate_left(9).to_le_bytes());
        }
    }
    bytes
}
fn wire(order: &[usize], values: [f32; 6]) -> Vec<u8> {
    let mut bytes = vec![1, 1, 1];
    bytes.extend(float_bytes(order, values));
    bytes.extend([0, 0]);
    bytes
}
fn transform(root: &FigValue) -> &FigValue {
    root.get_array("nodeChanges").unwrap()[0]
        .get("transform")
        .unwrap()
}
fn object(values: [f32; 6], reverse: bool) -> FigValue {
    let mut fields = FIELDS
        .iter()
        .zip(values)
        .map(|(name, value)| ((*name).into(), FigValue::Float(value)))
        .collect::<Vec<_>>();
    if reverse {
        fields.reverse();
    }
    FigValue::Object(fields.into())
}

#[test]
fn compact_affine_permutations_preserve_values_and_full_decoder_objects() {
    let values = [1.25, -0.25, 16.5, 0.125, 0.875, -12.25];
    for order in [[0, 1, 2, 3, 4, 5], [5, 3, 4, 0, 2, 1]] {
        let schema = schema(&order);
        let bytes = wire(&order, values);
        let full = decode_message(&schema, &bytes).unwrap();
        let imported = decode_import_message(&schema, &bytes).unwrap();
        assert!(matches!(transform(&full), FigValue::Object(_)));
        assert!(matches!(transform(&imported), FigValue::Affine(_)));
        assert_eq!(full, imported);
        assert!(transform(&imported).is_object());
        for (index, name) in FIELDS.iter().enumerate() {
            assert_eq!(
                transform(&imported).get_f64(name).unwrap().to_bits(),
                f64::from(values[index]).to_bits()
            );
        }
    }
}

#[test]
fn compact_affine_requires_six_unique_native_float_fields_without_extensions() {
    let base = schema(&[0, 1, 2, 3, 4, 5]);
    for change in 0..5 {
        let mut schema = base.clone();
        match change {
            0 => schema.definitions[0].kind = DefKind::Message,
            1 => schema.definitions[0].fields[0].type_name = "uint".into(),
            2 => schema.definitions[0].fields[0].is_array = true,
            3 => schema.definitions[0].fields[0].name = "m01".into(),
            _ => schema.definitions[0]
                .fields
                .push(field("extension", "float", false, 7)),
        }
        assert!(definitions(&schema)[0].is_none());
    }
}

#[test]
fn compact_affine_mutations_keep_float_updates_inline_and_materialize_other_changes() {
    let values = [1.25, -0.25, 16.5, 0.125, 0.875, -12.25];
    let mut affine = FigValue::Affine(Box::new(values));
    affine.set("m02", FigValue::Float(-0.0));
    assert!(matches!(affine, FigValue::Affine(_)));
    assert_eq!(
        affine.get_f64("m02").unwrap().to_bits(),
        (-0.0f64).to_bits()
    );
    assert_eq!(affine.get_f64("m12"), Some(-12.25));
    *affine.get_mut("m12").unwrap() = FigValue::Float(7.0);
    assert!(matches!(affine, FigValue::Object(_)));
    assert_eq!(affine.get_f64("m00"), Some(1.25));
    let mut affine = FigValue::Affine(Box::new(values));
    affine.set("extension", FigValue::Str("retained".into()));
    assert_eq!(affine.get_str("extension"), Some("retained"));
    assert_eq!(affine.get_f64("m11"), Some(0.875));
    let mut affine = FigValue::Affine(Box::new(values));
    affine.set("m00", FigValue::Uint(9));
    assert!(matches!(affine.get("m00"), Some(FigValue::Uint(9))));
    assert_eq!(affine.get_f64("m12"), Some(-12.25));
}

#[test]
fn compact_affine_value_equality_normalizes_order_but_preserves_float_contracts() {
    let values = [1.25, -0.25, 16.5, 0.125, 0.875, -12.25];
    let (compact, ordinary, reversed) = (
        FigValue::Affine(Box::new(values)),
        object(values, false),
        object(values, true),
    );
    assert_eq!(compact, ordinary);
    assert_eq!(ordinary, reversed);
    assert_eq!(compact, reversed);
    let zero = FigValue::Affine(Box::new([0.0; 6]));
    let mut minus = [0.0; 6];
    minus[0] = -0.0;
    assert_eq!(zero, FigValue::Affine(Box::new(minus)));
    assert_eq!(
        FigValue::Affine(Box::new(minus))
            .get_f64("m00")
            .unwrap()
            .to_bits(),
        (-0.0f64).to_bits()
    );
    let mut nan = values;
    nan[0] = f32::from_bits(0x7fc01234);
    assert_ne!(
        FigValue::Affine(Box::new(nan)),
        FigValue::Affine(Box::new(nan))
    );
    assert_ne!(FigValue::Affine(Box::new(nan)), object(nan, false));
}

#[test]
fn compact_affine_wire_and_projection_validation_match_the_generic_path() {
    let order = [0, 1, 2, 3, 4, 5];
    let schema = schema(&order);
    let values = [1.25, -0.25, 16.5, 0.125, 0.875, -12.25];
    let mut trailing = wire(&order, values);
    trailing.push(0);
    assert!(decode_import_message(&schema, &trailing).is_err());
    assert!(decode_import_message(&schema, &wire(&order, values)[..5]).is_err());
    let raw = float_bytes(&order, values);
    for scope in [DecodeScope::Full, DecodeScope::Import] {
        let mut decoder = Decoder::new(&schema, &raw).unwrap();
        assert!(matches!(
            decoder.decode_struct(0, MAX_DEPTH, scope),
            Err(KiwiError::TooDeep)
        ));
    }
    let mut decoder = Decoder::new(&schema, &raw).unwrap();
    assert!(
        matches!(decoder.decode_struct(0, 0, DecodeScope::PointEvidence).unwrap(), FigValue::Object(fields) if fields.is_empty())
    );
    let special = [
        f32::from_bits(0x7fc01234),
        f32::INFINITY,
        f32::NEG_INFINITY,
        0.0,
        1.0,
        2.0,
    ];
    let bytes = wire(&order, special);
    let full = decode_message(&schema, &bytes).unwrap();
    let imported = decode_import_message(&schema, &bytes).unwrap();
    for name in FIELDS {
        assert_eq!(
            transform(&full).get_f64(name).unwrap().to_bits(),
            transform(&imported).get_f64(name).unwrap().to_bits()
        );
    }
}
