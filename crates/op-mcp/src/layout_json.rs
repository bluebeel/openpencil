//! Borrowed layout serialization; never builds a duplicate serde_json::Value tree.
use super::{js_number, LayoutRecord};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};
use std::collections::HashMap;

type Index<'r> = HashMap<&'r str, Vec<&'r LayoutRecord>>;

pub(super) fn serialize_layout(
    records: &[&LayoutRecord],
    parent: &str,
    offset: (f64, f64),
) -> String {
    let mut index: Index<'_> = HashMap::new();
    for &record in records {
        index
            .entry(record.ancestors.last().map(String::as_str).unwrap_or(""))
            .or_default()
            .push(record);
    }
    let roots = index.get(parent).map(Vec::as_slice).unwrap_or(&[]);
    // Match the active serde_json::Map backend: insertion order with its
    // preserve_order feature, lexical order otherwise. Allocate only8 keys once.
    let mut order = serde_json::Map::new();
    for key in [
        "id", "name", "type", "x", "y", "width", "height", "children",
    ] {
        order.insert(key.into(), serde_json::Value::Null);
    }
    let order: Vec<_> = order.into_iter().map(|(key, _)| key).collect();
    let entries = Entries {
        records: roots,
        index: &index,
        offset,
        order: &order,
    };
    #[derive(Serialize)]
    struct Response<T> {
        layout: T,
    }
    serde_json::to_string(&Response { layout: entries }).expect("in-memory computed layout JSON")
}

struct Entries<'a, 'r> {
    records: &'a [&'r LayoutRecord],
    index: &'a Index<'r>,
    offset: (f64, f64),
    order: &'a [String],
}
impl Serialize for Entries<'_, '_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.records.len()))?;
        for record in self.records {
            seq.serialize_element(&Entry {
                record,
                index: self.index,
                offset: self.offset,
                order: self.order,
            })?;
        }
        seq.end()
    }
}

struct Entry<'a, 'r> {
    record: &'r LayoutRecord,
    index: &'a Index<'r>,
    offset: (f64, f64),
    order: &'a [String],
}
impl Serialize for Entry<'_, '_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let r = self.record;
        let children = self
            .index
            .get(r.id.as_str())
            .filter(|items| !items.is_empty());
        let mut map = serializer.serialize_map(Some(
            6 + usize::from(r.name.is_some()) + usize::from(children.is_some()),
        ))?;
        for key in self.order {
            match key.as_str() {
                "children" => {
                    if let Some(children) = children {
                        map.serialize_entry(
                            key,
                            &Entries {
                                records: children,
                                index: self.index,
                                offset: self.offset,
                                order: self.order,
                            },
                        )?;
                    }
                }
                "height" => map.serialize_entry(key, &js_number(r.h))?,
                "id" => map.serialize_entry(key, &r.id)?,
                "name" => {
                    if let Some(name) = &r.name {
                        map.serialize_entry(key, name)?;
                    }
                }
                "type" => map.serialize_entry(key, &r.type_label)?,
                "width" => map.serialize_entry(key, &js_number(r.w))?,
                "x" => map.serialize_entry(key, &js_number(r.x - self.offset.0))?,
                "y" => map.serialize_entry(key, &js_number(r.y - self.offset.1))?,
                _ => unreachable!("layout key prototype"),
            }
        }
        map.end()
    }
}

#[cfg(test)]
#[path = "layout_json_tests.rs"]
mod tests;
