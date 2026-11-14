//! Borrowed, bounded allocation forecasts; no JSON buffer or per-header quota.
use super::{node_wire, PenNode, SharedNode};
use serde::{
    ser::{
        SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
        SerializeTupleStruct, SerializeTupleVariant,
    },
    Serialize, Serializer,
};

pub const COMMAND_CAPTURE_ALLOWANCE: usize = 2 * 1024 * 1024 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureSizeError {
    Capacity { attempted: usize, limit: usize },
    Serialization,
}
impl std::fmt::Display for CaptureSizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Capacity { attempted, limit } => write!(
                f,
                "command capture forecast {attempted} exceeds {limit} accounted bytes"
            ),
            Self::Serialization => f.write_str("command capture metadata could not be counted"),
        }
    }
}
impl std::error::Error for CaptureSizeError {}
impl serde::ser::Error for CaptureSizeError {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Self::Serialization
    }
}
pub fn command_capture_node_bytes(
    node: &PenNode,
    header_only: bool,
    limit: usize,
) -> Result<usize, CaptureSizeError> {
    let mut count = Counter {
        bytes: 0,
        limit,
        key: false,
    };
    count.add(std::mem::size_of::<SharedNode>() * 4)?;
    if header_only {
        node.serialize(node_wire::ReplaceChildren::new(&mut count, Some(&[])))?;
    } else {
        node.serialize(&mut count)?;
    }
    Ok(count.bytes)
}
pub fn command_capture_value_bytes(
    value: &impl Serialize,
    limit: usize,
) -> Result<usize, CaptureSizeError> {
    let mut count = Counter {
        bytes: 0,
        limit,
        key: false,
    };
    value.serialize(&mut count)?;
    Ok(count.bytes)
}
struct Counter {
    bytes: usize,
    limit: usize,
    key: bool,
}
impl Counter {
    fn add(&mut self, bytes: usize) -> Result<(), CaptureSizeError> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > self.limit {
            Err(CaptureSizeError::Capacity {
                attempted: self.bytes,
                limit: self.limit,
            })
        } else {
            Ok(())
        }
    }
    fn container(
        &mut self,
        slots: Option<usize>,
        slot: usize,
    ) -> Result<Compound<'_>, CaptureSizeError> {
        self.add(128usize.saturating_add(slots.unwrap_or(0).saturating_mul(slot)))?;
        Ok(Compound(self))
    }
}
struct Compound<'a>(&'a mut Counter);
macro_rules! scalar {($($method:ident($ty:ty)),*$(,)?)=>{$(fn $method(self,_:$ty)->Result<(),CaptureSizeError>{self.add(32)})*};}
impl<'a> Serializer for &'a mut Counter {
    type Ok = ();
    type Error = CaptureSizeError;
    type SerializeSeq = Compound<'a>;
    type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>;
    type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = Compound<'a>;
    type SerializeStruct = Compound<'a>;
    type SerializeStructVariant = Compound<'a>;
    scalar! {serialize_bool(bool),serialize_i8(i8),serialize_i16(i16),serialize_i32(i32),serialize_i64(i64),serialize_i128(i128),serialize_u8(u8),serialize_u16(u16),serialize_u32(u32),serialize_u64(u64),serialize_u128(u128),serialize_f32(f32),serialize_f64(f64),serialize_char(char)}
    fn serialize_str(self, value: &str) -> Result<(), Self::Error> {
        if self.key && value == "type" {
            self.add(std::mem::size_of::<SharedNode>() * 4)?;
        }
        self.add(64usize.saturating_add(value.len().saturating_mul(8)))
    }
    fn serialize_bytes(self, value: &[u8]) -> Result<(), Self::Error> {
        self.add(64usize.saturating_add(value.len().saturating_mul(8)))
    }
    fn serialize_none(self) -> Result<(), Self::Error> {
        self.add(8)
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<(), Self::Error> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), Self::Error> {
        self.add(8)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Self::Error> {
        self.add(8)
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        value: &'static str,
    ) -> Result<(), Self::Error> {
        self.serialize_str(value)
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        _: u32,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.serialize_str(key)?;
        value.serialize(self)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.container(len, 32)
    }
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.container(Some(len), 32)
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.container(Some(len), 32)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.container(Some(len), 32)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.container(len, 128)
    }
    fn serialize_struct(
        self,
        _: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.container(Some(len), 64)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.container(Some(len), 64)
    }
    fn collect_str<T: ?Sized + std::fmt::Display>(self, value: &T) -> Result<(), Self::Error> {
        struct Length(usize);
        impl std::fmt::Write for Length {
            fn write_str(&mut self, value: &str) -> std::fmt::Result {
                self.0 = self.0.saturating_add(value.len());
                Ok(())
            }
        }
        let mut length = Length(0);
        std::fmt::write(&mut length, format_args!("{value}"))
            .map_err(|_| CaptureSizeError::Serialization)?;
        self.add(64usize.saturating_add(length.0.saturating_mul(8)))
    }
    fn is_human_readable(&self) -> bool {
        true
    }
}
macro_rules! sequence {
    ($trait:ident,$method:ident) => {
        impl $trait for Compound<'_> {
            type Ok = ();
            type Error = CaptureSizeError;
            fn $method<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
                value.serialize(&mut *self.0)
            }
            fn end(self) -> Result<(), Self::Error> {
                Ok(())
            }
        }
    };
}
sequence!(SerializeSeq, serialize_element);
sequence!(SerializeTuple, serialize_element);
sequence!(SerializeTupleStruct, serialize_field);
sequence!(SerializeTupleVariant, serialize_field);
impl SerializeMap for Compound<'_> {
    type Ok = ();
    type Error = CaptureSizeError;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Self::Error> {
        self.0.add(64)?;
        self.0.key = true;
        let result = key.serialize(&mut *self.0);
        self.0.key = false;
        result
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        value.serialize(&mut *self.0)
    }
    fn end(self) -> Result<(), Self::Error> {
        Ok(())
    }
}
macro_rules! structure {
    ($trait:ident) => {
        impl $trait for Compound<'_> {
            type Ok = ();
            type Error = CaptureSizeError;
            fn serialize_field<T: ?Sized + Serialize>(
                &mut self,
                key: &'static str,
                value: &T,
            ) -> Result<(), Self::Error> {
                SerializeMap::serialize_key(self, key)?;
                value.serialize(&mut *self.0)
            }
            fn end(self) -> Result<(), Self::Error> {
                Ok(())
            }
        }
    };
}
structure!(SerializeStruct);
structure!(SerializeStructVariant);
