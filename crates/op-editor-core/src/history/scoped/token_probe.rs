//! Borrowed, allocation-free conservative page dependency detection.
use serde::{ser::*, Serialize};
use std::fmt;

#[derive(Debug)]
enum ProbeError {
    Found,
    Other,
}
impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Found => "token found",
            Self::Other => "token probe failed",
        })
    }
}
impl std::error::Error for ProbeError {}
impl serde::ser::Error for ProbeError {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Self::Other
    }
}
pub(crate) fn contains<T: Serialize + ?Sized>(value: &T, token: &str) -> bool {
    // An unsupported custom serializer conservatively captures the page.
    jian_ops_schema::image_table::without_save_scope(|| value.serialize(Probe(token))).is_err()
}
struct Probe<'a>(&'a str);
impl Probe<'_> {
    fn child<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), ProbeError> {
        value.serialize(Probe(self.0))
    }
}
macro_rules! plain {($($name:ident($ty:ty));*$(;)?)=>{$(fn $name(self,_:$ty)->Result<(),Self::Error>{Ok(())})*}}
impl<'a> serde::Serializer for Probe<'a> {
    type Ok = ();
    type Error = ProbeError;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;
    plain! {serialize_bool(bool);serialize_i8(i8);serialize_i16(i16);serialize_i32(i32);serialize_i64(i64);serialize_i128(i128);serialize_u8(u8);serialize_u16(u16);serialize_u32(u32);serialize_u64(u64);serialize_u128(u128);serialize_f32(f32);serialize_f64(f64);serialize_char(char);serialize_bytes(&[u8]);}
    fn serialize_str(self, value: &str) -> Result<(), Self::Error> {
        if value == self.0 {
            Err(ProbeError::Found)
        } else {
            Ok(())
        }
    }
    fn serialize_none(self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(mut self, value: &T) -> Result<(), Self::Error> {
        self.child(value)
    }
    fn serialize_unit(self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Self::Error> {
        Ok(())
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.child(value)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        mut self,
        _: &'static str,
        _: u32,
        _: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.child(value)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self, Self::Error> {
        Ok(self)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self, Self::Error> {
        Ok(self)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, Self::Error> {
        Ok(self)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Self::Error> {
        Ok(self)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self, Self::Error> {
        Ok(self)
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, Self::Error> {
        Ok(self)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Self::Error> {
        Ok(self)
    }
    fn collect_str<T: fmt::Display + ?Sized>(self, value: &T) -> Result<(), Self::Error> {
        struct Match<'a> {
            token: &'a str,
            pos: usize,
            equal: bool,
        }
        impl fmt::Write for Match<'_> {
            fn write_str(&mut self, text: &str) -> fmt::Result {
                self.equal &= self
                    .token
                    .as_bytes()
                    .get(self.pos..self.pos.saturating_add(text.len()))
                    == Some(text.as_bytes());
                self.pos = self.pos.saturating_add(text.len());
                Ok(())
            }
        }
        let mut matcher = Match {
            token: self.0,
            pos: 0,
            equal: true,
        };
        fmt::write(&mut matcher, format_args!("{value}")).map_err(|_| ProbeError::Other)?;
        if matcher.equal && matcher.pos == self.0.len() {
            Err(ProbeError::Found)
        } else {
            Ok(())
        }
    }
}
macro_rules! sequence {($($trait:ident::$method:ident);*$(;)?)=>{$(impl $trait for Probe<'_>{type Ok=();type Error=ProbeError;fn $method<T:Serialize+?Sized>(&mut self,v:&T)->Result<(),Self::Error>{self.child(v)}fn end(self)->Result<(),Self::Error>{Ok(())}})*}}
sequence! {SerializeSeq::serialize_element;SerializeTuple::serialize_element;SerializeTupleStruct::serialize_field;SerializeTupleVariant::serialize_field;}
impl SerializeMap for Probe<'_> {
    type Ok = ();
    type Error = ProbeError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), Self::Error> {
        self.child(v)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), Self::Error> {
        self.child(v)
    }
    fn end(self) -> Result<(), Self::Error> {
        Ok(())
    }
}
impl SerializeStruct for Probe<'_> {
    type Ok = ();
    type Error = ProbeError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        v: &T,
    ) -> Result<(), Self::Error> {
        self.child(v)
    }
    fn end(self) -> Result<(), Self::Error> {
        Ok(())
    }
}
impl SerializeStructVariant for Probe<'_> {
    type Ok = ();
    type Error = ProbeError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        v: &T,
    ) -> Result<(), Self::Error> {
        self.child(v)
    }
    fn end(self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_unknown_tokens_and_large_unrelated_strings_are_borrowed() {
        let value = serde_json::json!({"future":{"nested":[{"fill":"$old"}]},"image":"x".repeat(1024*1024)});
        assert!(contains(&value, "$old"));
        assert!(!contains(&value, "$other"));
        assert!(!contains(&serde_json::json!({"fill":"prefix$old"}), "$old"));
    }
}
