//! Serialize requested tree depth without visiting omitted descendants.
//!
//! Keep the schema's own serializer for every field. Only the node's top-level
//! `children` entry is substituted, so new schema fields need no parallel list.
use jian_ops_schema::node::PenNode;
use serde::{ser, Serialize, Serializer};
use serde_json::{Map, Value};

pub(super) fn value(node: &PenNode, depth: i32) -> Value {
    try_value(node, depth).unwrap_or(Value::Null)
}

fn try_value(node: &PenNode, depth: i32) -> Result<Value, serde_json::Error> {
    if depth == -1 {
        return serde_json::to_value(node);
    }
    node.serialize(NodeSerializer {
        node,
        depth,
        fields: Map::new(),
        key: None,
    })
}

struct NodeSerializer<'a> {
    node: &'a PenNode,
    depth: i32,
    fields: Map<String, Value>,
    key: Option<String>,
}

impl NodeSerializer<'_> {
    fn field<T: ?Sized + Serialize>(
        &mut self,
        key: &str,
        field: &T,
    ) -> Result<(), serde_json::Error> {
        let value = if key == "children" {
            match self.node.children_ref() {
                Some([]) => Value::Array(Vec::new()),
                Some(_) if self.depth <= 0 => Value::String("...".into()),
                Some(children) => Value::Array(
                    children
                        .iter()
                        .map(|child| try_value(child, self.depth - 1))
                        .collect::<Result<_, _>>()?,
                ),
                None => serde_json::to_value(field)?,
            }
        } else {
            serde_json::to_value(field)?
        };
        self.fields.insert(key.into(), value);
        Ok(())
    }
}

impl ser::SerializeMap for NodeSerializer<'_> {
    type Ok = Value;
    type Error = serde_json::Error;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Self::Error> {
        self.key = Some(serde_json::from_value(serde_json::to_value(key)?)?);
        Ok(())
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        let key = self
            .key
            .take()
            .ok_or_else(|| ser::Error::custom("node field without key"))?;
        self.field(&key, value)
    }

    fn serialize_entry<K: ?Sized + Serialize, V: ?Sized + Serialize>(
        &mut self,
        key: &K,
        value: &V,
    ) -> Result<(), Self::Error> {
        let key: String = serde_json::from_value(serde_json::to_value(key)?)?;
        self.field(&key, value)
    }

    fn end(self) -> Result<Value, Self::Error> {
        Ok(Value::Object(self.fields))
    }
}

impl ser::SerializeStruct for NodeSerializer<'_> {
    type Ok = Value;
    type Error = serde_json::Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.field(key, value)
    }

    fn end(self) -> Result<Value, Self::Error> {
        Ok(Value::Object(self.fields))
    }
}

macro_rules! scalar {
    ($($name:ident($ty:ty)),* $(,)?) => {$(
        fn $name(self, value: $ty) -> Result<Value, Self::Error> {
            serde_json::value::Serializer.$name(value)
        }
    )*};
}

// PenNode currently serializes as a flattened tagged map. Forward every other
// serde shape unchanged, keeping this adapter independent of schema internals.
impl Serializer for NodeSerializer<'_> {
    type Ok = Value;
    type Error = serde_json::Error;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeSeq = <serde_json::value::Serializer as Serializer>::SerializeSeq;
    type SerializeTuple = <serde_json::value::Serializer as Serializer>::SerializeTuple;
    type SerializeTupleStruct = <serde_json::value::Serializer as Serializer>::SerializeTupleStruct;
    type SerializeTupleVariant =
        <serde_json::value::Serializer as Serializer>::SerializeTupleVariant;
    type SerializeStructVariant =
        <serde_json::value::Serializer as Serializer>::SerializeStructVariant;

    scalar!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_f32(f32),
        serialize_f64(f64),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8])
    );

    fn serialize_none(self) -> Result<Value, Self::Error> {
        serde_json::value::Serializer.serialize_none()
    }
    fn serialize_unit(self) -> Result<Value, Self::Error> {
        serde_json::value::Serializer.serialize_unit()
    }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<Value, Self::Error> {
        serde_json::value::Serializer.serialize_some(v)
    }
    fn serialize_unit_struct(self, name: &'static str) -> Result<Value, Self::Error> {
        serde_json::value::Serializer.serialize_unit_struct(name)
    }
    fn serialize_unit_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
    ) -> Result<Value, Self::Error> {
        serde_json::value::Serializer.serialize_unit_variant(name, index, variant)
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        v: &T,
    ) -> Result<Value, Self::Error> {
        serde_json::value::Serializer.serialize_newtype_struct(name, v)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        v: &T,
    ) -> Result<Value, Self::Error> {
        serde_json::value::Serializer.serialize_newtype_variant(name, index, variant, v)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        serde_json::value::Serializer.serialize_seq(len)
    }
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        serde_json::value::Serializer.serialize_tuple(len)
    }
    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        serde_json::value::Serializer.serialize_tuple_struct(name, len)
    }
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        serde_json::value::Serializer.serialize_tuple_variant(name, index, variant, len)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self, Self::Error> {
        Ok(self)
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, Self::Error> {
        Ok(self)
    }
    fn serialize_struct_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        serde_json::value::Serializer.serialize_struct_variant(name, index, variant, len)
    }
}

#[cfg(test)]
#[path = "node_snapshot_tests.rs"]
mod tests;
