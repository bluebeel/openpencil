use super::*;
use crate::kiwi::{
    decode_import_message, decode_message, DefKind, Definition, Field, FigKey, FigValue, Schema,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[test]
fn packed_backing_mutation_matches_vec_and_clones_own_their_elements() {
    let mut input = Vec::with_capacity(256);
    input.extend([1, 2, 3]);
    let mut packed: PackedVec<_> = input.into();
    assert_eq!(packed.capacity(), 3);
    let clone = packed.clone();
    packed.push(4);
    packed.extend([5, 6]);
    packed.insert(1, 9);
    assert_eq!(packed.remove(2), 2);
    packed.retain(|v| v % 2 != 0);
    assert_eq!(&*packed, &[1, 9, 3, 5]);
    packed.truncate(2);
    assert_eq!(packed.capacity(), 2);
    assert_eq!(Vec::from(packed), vec![1, 9]);
    assert_eq!(&*clone, &[1, 2, 3]);
}

#[test]
fn packed_mutation_keeps_all_survivors_on_panic_and_drops_exactly_once() {
    struct Item(Arc<AtomicUsize>);
    impl Drop for Item {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    let mut values: PackedVec<_> = (0..4).map(|_| Item(drops.clone())).collect();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        values.retain(|_| panic!("callback"));
    }));
    assert!(result.is_err());
    assert_eq!(values.len(), 4);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    values.clear();
    assert_eq!(drops.load(Ordering::SeqCst), 4);
}

#[test]
fn packed_objects_keep_field_order_and_mutable_arrays() {
    let mut object = FigValue::Object(
        vec![
            ("unknown".into(), FigValue::Bytes(vec![0, 255].into())),
            (
                "items".into(),
                FigValue::Array(vec![FigValue::Uint(7)].into()),
            ),
        ]
        .into(),
    );
    object
        .get_array_mut("items")
        .unwrap()
        .push(FigValue::Uint(8));
    object.set("added", FigValue::Str("new".into()));
    let clone = object.clone();
    object.get_array_mut("items").unwrap().clear();
    assert_eq!(
        clone.get_array("items").unwrap(),
        &[FigValue::Uint(7), FigValue::Uint(8)]
    );
    let FigValue::Object(fields) = clone else {
        panic!()
    };
    assert_eq!(
        fields.iter().map(|(k, _)| k.as_ref()).collect::<Vec<_>>(),
        ["unknown", "items", "added"]
    );
}

fn enum_schema() -> Schema {
    let field = |name: &str, kind: &str, id| Field {
        name: name.into(),
        type_name: kind.into(),
        is_array: false,
        value: id,
    };
    Schema {
        definitions: vec![
            Definition {
                name: "Mode".into(),
                kind: DefKind::Enum,
                fields: vec![field("FUTURE_MODE", "", 1)],
            },
            Definition {
                name: "Message".into(),
                kind: DefKind::Message,
                fields: vec![field("a", "Mode", 1), field("b", "Mode", 2)],
            },
        ],
    }
}

#[test]
fn enum_symbols_share_storage_but_generic_decode_keeps_strings() {
    let schema = enum_schema();
    let bytes = [1, 1, 2, 1, 0];
    let full = decode_message(&schema, &bytes).unwrap();
    let imported = decode_import_message(&schema, &bytes).unwrap();
    assert_eq!(full, imported);
    assert!(matches!(full.get("a"), Some(FigValue::Str(_))));
    let (Some(FigValue::Symbol(a)), Some(FigValue::Symbol(b))) =
        (imported.get("a"), imported.get("b"))
    else {
        panic!()
    };
    assert!(FigKey::ptr_eq(a, b));
    assert_eq!(imported.get_str("a"), Some("FUTURE_MODE"));
    assert_eq!(
        FigValue::Symbol(a.clone()),
        FigValue::Str("FUTURE_MODE".into())
    );
    assert_eq!(
        decode_import_message(&schema, &[1, 99, 0])
            .unwrap()
            .get("a"),
        None
    );
    assert!(decode_import_message(&schema, &[1, 1]).is_err());
}
