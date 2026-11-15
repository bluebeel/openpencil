use super::*;
use crate::kiwi::{decode_import_message, decode_message, DefKind, Definition, Field};

fn fixture() -> (Schema, Vec<u8>) {
    fn field(name: &str, kind: &str, array: bool, id: u32) -> Field {
        Field {
            name: name.into(),
            type_name: kind.into(),
            is_array: array,
            value: id,
        }
    }
    let schema = Schema {
        definitions: vec![
            Definition {
                name: "Cache".into(),
                kind: DefKind::Message,
                fields: vec![
                    field("payload", "string", false, 1),
                    field("future", "uint", true, 2),
                ],
            },
            Definition {
                name: "NodeChange".into(),
                kind: DefKind::Message,
                fields: vec![field("derivedSymbolData", "Cache", true, 1)],
            },
            Definition {
                name: "Message".into(),
                kind: DefKind::Message,
                fields: vec![field("nodeChanges", "NodeChange", true, 1)],
            },
        ],
    };
    let mut wire = vec![1, 1, 1, 2];
    for tag in [b'A', b'B'] {
        wire.push(1);
        wire.extend(std::iter::repeat_n(tag, 300));
        wire.push(0);
        wire.extend([2, 3, 7, 8, 9, 0]);
    }
    wire.extend([0, 0]);
    (schema, wire)
}
fn node(root: &FigValue) -> &FigValue {
    &root.get_array("nodeChanges").unwrap()[0]
}
fn cell(root: &FigValue) -> &Arc<Deferred> {
    let FigValue::Object(fields) = node(root) else {
        panic!()
    };
    let FigValue::Deferred(cell) = &fields
        .iter()
        .find(|(k, _)| k.as_ref() == "derivedSymbolData")
        .unwrap()
        .1
    else {
        panic!("cache should remain deferred")
    };
    cell
}
fn mutate_node(root: &mut FigValue) -> &mut FigValue {
    &mut root.get_array_mut("nodeChanges").unwrap()[0]
}
fn eager(root: &FigValue) -> &[FigValue] {
    node(root).get_array("derivedSymbolData").unwrap()
}

#[test]
fn validation_full_fields_and_backing_release() {
    let (schema, wire) = fixture();
    let lazy = decode_import_message(&schema, &wire).unwrap();
    assert!(cell(&lazy).ready().is_none());
    let (context, bytes) = {
        let pending = cell(&lazy).pending.lock().unwrap();
        let Some(Pending::Wire { shared, bytes, .. }) = pending.as_ref() else {
            panic!()
        };
        (Arc::downgrade(shared), Arc::downgrade(bytes))
    };
    let full = decode_message(&schema, &wire).unwrap();
    assert_eq!(lazy, full);
    assert_eq!(
        eager(&lazy)[0].get_array("future").unwrap(),
        &[FigValue::Uint(7), FigValue::Uint(8), FigValue::Uint(9)]
    );
    assert_eq!(cell(&lazy).pending_bytes(), 0);
    assert!(context.upgrade().is_none());
    assert!(bytes.upgrade().is_none());
    for end in [wire.len() - 1, wire.len() / 2, 8] {
        assert!(decode_import_message(&schema, &wire[..end]).is_err());
    }
    let mut invalid = wire.clone();
    invalid[1] = 255;
    assert!(decode_import_message(&schema, &invalid).is_err());
}
#[test]
fn unique_move_and_shared_copy_on_write() {
    let (schema, wire) = fixture();
    let mut unique = decode_import_message(&schema, &wire).unwrap();
    let address = eager(&unique).as_ptr() as usize;
    let array = mutate_node(&mut unique)
        .get_array_mut("derivedSymbolData")
        .unwrap();
    assert_eq!(
        array.as_ptr() as usize,
        address,
        "unique ownership moves the decoded backing"
    );
    let original = decode_import_message(&schema, &wire).unwrap();
    let mut edited = original.clone();
    mutate_node(&mut edited)
        .get_array_mut("derivedSymbolData")
        .unwrap()[0]
        .set("payload", FigValue::Str("edited".into()));
    assert_eq!(eager(&edited)[0].get_str("payload"), Some("edited"));
    assert_eq!(eager(&original)[0].get_str("payload").unwrap().len(), 300);
    assert_eq!(original, decode_message(&schema, &wire).unwrap());
}
#[test]
fn deferred_transform_order_and_shared_isolation() {
    let (schema, wire) = fixture();
    let original = decode_import_message(&schema, &wire).unwrap();
    let mut changed = cell(&original).clone();
    Deferred::map(&mut changed, 0, |value| {
        let FigValue::Array(values) = value else {
            panic!()
        };
        values[0].set("extension", FigValue::Uint(1));
    });
    Deferred::map(&mut changed, 0, |value| {
        let FigValue::Array(values) = value else {
            panic!()
        };
        assert_eq!(values[0].get_f64("extension"), Some(1.0));
        values[0].set("extension", FigValue::Uint(2));
    });
    assert!(changed.ready().is_none());
    assert_eq!(
        changed.get().as_array().unwrap()[0].get_f64("extension"),
        Some(2.0)
    );
    assert!(eager(&original)[0].get("extension").is_none());
    assert_eq!(
        changed.get().as_array().unwrap()[0].get_array("future"),
        eager(&original)[0].get_array("future")
    );
}
#[test]
fn concurrent_read_and_conservative_bound() {
    fn sharing<T: Send + Sync>() {}
    sharing::<FigValue>();
    let (schema, wire) = fixture();
    let value = Arc::new(decode_import_message(&schema, &wire).unwrap());
    let before = cell(&value).retained_bound(&crate::instance::value_retained_bound);
    assert!(before > cell(&value).pending_bytes() + std::mem::size_of::<Deferred>());
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let tasks = (0..8)
        .map(|_| {
            let value = value.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                eager(&value).as_ptr() as usize
            })
        })
        .collect::<Vec<_>>();
    let addresses = tasks
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        addresses.len(),
        1,
        "OnceLock installs one immutable decoded view"
    );
    assert_eq!(cell(&value).pending_bytes(), 0);
    assert!(cell(&value).retained_bound(&crate::instance::value_retained_bound) < before);
}
