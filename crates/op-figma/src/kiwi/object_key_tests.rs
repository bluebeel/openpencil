use super::*;
use crate::kiwi::FigValue;
use std::collections::{hash_map::DefaultHasher, HashMap};
use std::hash::{Hash, Hasher};

#[test]
fn thin_key_layout_preserves_value_size_and_thread_sharing() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<FigKey>();
    assert_send_sync::<FigValue>();
    assert_eq!(std::mem::size_of::<FigKey>(), std::mem::size_of::<usize>());
    #[cfg(target_pointer_width = "64")]
    {
        assert_eq!(std::mem::size_of::<FigValue>(), 32);
        assert_eq!(std::mem::size_of::<(FigKey, FigValue)>(), 40);
    }
}

#[test]
fn key_conversions_preserve_utf8_value_hash_borrowing_and_debug() {
    for text in ["", "color", "café", "e\u{301}", "x\0y", "quoted\"key"] {
        let forms = [
            FigKey::from(text),
            FigKey::from(text.to_owned()),
            FigKey::from(Box::<str>::from(text)),
            FigKey::from(Arc::<str>::from(text)),
            FigKey::from(std::borrow::Cow::Borrowed(text)),
            FigKey::from(std::borrow::Cow::Owned(text.to_owned())),
        ];
        for key in forms {
            assert_eq!(key.as_ref(), text);
            assert_eq!(&*key, text);
            assert_eq!(key.len(), text.len());
            assert_eq!(format!("{key:?}"), format!("{text:?}"));
            assert_eq!(key.to_string(), text);
            assert_eq!(key, FigKey::from(text));
            let (mut actual, mut expected) = (DefaultHasher::new(), DefaultHasher::new());
            key.hash(&mut actual);
            text.hash(&mut expected);
            assert_eq!(actual.finish(), expected.finish());
            let mut map = HashMap::new();
            map.insert(key, 7);
            assert_eq!(map.get(text), Some(&7));
        }
    }
    assert_ne!(FigKey::from("é"), FigKey::from("e\u{301}"));
}

#[test]
fn clones_share_owned_key_storage_without_borrowing_the_input() {
    let source = format!("dynamic-{}", 42);
    let key = FigKey::from(source.as_str());
    let clone = key.clone();
    assert!(FigKey::ptr_eq(&key, &clone));
    assert_eq!(Arc::strong_count(&key.0), 2);
    drop(source);
    drop(key);
    assert_eq!(clone.as_ref(), "dynamic-42");
    assert_eq!(Arc::strong_count(&clone.0), 1);
}

#[test]
fn generated_object_fields_keep_lookup_mutation_and_clone_semantics() {
    let name = format!("field-{}", 123);
    let key = FigKey::from(name.as_str());
    let mut value = FigValue::Object(vec![(key, FigValue::Float(1.0))]);
    drop(name);
    assert_eq!(value.get_f64("field-123"), Some(1.0));
    value.set("field-123", FigValue::Float(2.0));
    let FigValue::Object(fields) = &value else {
        unreachable!()
    };
    assert_eq!(
        fields.len(),
        1,
        "updating an existing field does not append a key"
    );
    let clone = value.clone();
    let (FigValue::Object(left), FigValue::Object(right)) = (&value, &clone) else {
        unreachable!()
    };
    assert!(FigKey::ptr_eq(&left[0].0, &right[0].0));
    value.set("", FigValue::Bool(true));
    value.set("x\0y", FigValue::Str("retained".into()));
    *value.get_mut("field-123").unwrap() = FigValue::Float(-0.0);
    assert_eq!(
        value.get_f64("field-123").unwrap().to_bits(),
        (-0.0f64).to_bits()
    );
    assert_eq!(value.get_bool(""), Some(true));
    assert_eq!(value.get_str("x\0y"), Some("retained"));
    drop(value);
    assert_eq!(clone.get_f64("field-123"), Some(2.0));
    assert!(clone.get("").is_none());
}
