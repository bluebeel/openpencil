//! Exact distinctions omitted by ordinary PartialEq: numeric representation and
//! serialized map order. Equality still checks every ordinary string/value;
//! this stream therefore skips string payloads (including shared base64 images).
use serde::{
    ser::{
        SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
        SerializeTupleStruct, SerializeTupleVariant,
    },
    Serialize, Serializer,
};
use std::fmt::Write as _;
pub(super) fn write<T: Serialize + ?Sized>(value: &T, out: &mut Vec<u8>) {
    out.clear();
    jian_ops_schema::image_table::without_save_scope(|| {
        value.serialize(Probe {
            out,
            key: false,
            ends: 1,
        })
    })
    .expect("canonical metadata sensitivity");
}
pub(super) fn bytes<T: Serialize + ?Sized>(value: &T) -> Box<[u8]> {
    let mut out = Vec::new();
    write(value, &mut out);
    out.into_boxed_slice()
}
struct Probe<'a> {
    out: &'a mut Vec<u8>,
    key: bool,
    ends: u8,
}
impl Probe<'_> {
    fn token(&mut self, tag: u8, bytes: &[u8]) {
        self.out.push(tag);
        self.out
            .extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        self.out.extend_from_slice(bytes)
    }
    fn child<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
        key: bool,
    ) -> Result<(), serde_json::Error> {
        value.serialize(Probe {
            out: self.out,
            key,
            ends: 1,
        })
    }
    fn finish(self) {
        for _ in 0..self.ends {
            self.out.push(0)
        }
    }
}
macro_rules! number {($($name:ident($ty:ty)=$tag:expr);*$(;)?)=>{$(fn $name(mut self,value:$ty)->Result<(),Self::Error>{self.token($tag,&value.to_le_bytes());Ok(())})*}}
impl<'a> Serializer for Probe<'a> {
    type Ok = ();
    type Error = serde_json::Error;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;
    number! {serialize_i8(i8)=1;serialize_i16(i16)=2;serialize_i32(i32)=3;serialize_i64(i64)=4;serialize_i128(i128)=5;serialize_u8(u8)=6;serialize_u16(u16)=7;serialize_u32(u32)=8;serialize_u64(u64)=9;serialize_u128(u128)=10;}
    fn serialize_bool(mut self, v: bool) -> Result<(), Self::Error> {
        self.token(11, &[u8::from(v)]);
        Ok(())
    }
    fn serialize_f32(mut self, v: f32) -> Result<(), Self::Error> {
        self.token(12, &v.to_bits().to_le_bytes());
        Ok(())
    }
    fn serialize_f64(mut self, v: f64) -> Result<(), Self::Error> {
        self.token(13, &v.to_bits().to_le_bytes());
        Ok(())
    }
    fn serialize_char(self, v: char) -> Result<(), Self::Error> {
        let mut buf = [0; 4];
        self.serialize_str(v.encode_utf8(&mut buf))
    }
    fn serialize_str(mut self, v: &str) -> Result<(), Self::Error> {
        if self.key {
            self.token(14, v.as_bytes())
        } else {
            self.out.push(15)
        }
        Ok(())
    }
    fn serialize_bytes(mut self, v: &[u8]) -> Result<(), Self::Error> {
        if self.key {
            self.token(16, v)
        } else {
            self.out.push(17)
        }
        Ok(())
    }
    fn serialize_none(self) -> Result<(), Self::Error> {
        self.out.push(18);
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(mut self, v: &T) -> Result<(), Self::Error> {
        self.out.push(19);
        self.child(v, self.key)
    }
    fn serialize_unit(self) -> Result<(), Self::Error> {
        self.out.push(20);
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Self::Error> {
        self.serialize_unit()
    }
    fn serialize_unit_variant(
        mut self,
        _: &'static str,
        _: u32,
        v: &'static str,
    ) -> Result<(), Self::Error> {
        self.token(21, v.as_bytes());
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        mut self,
        _: &'static str,
        v: &T,
    ) -> Result<(), Self::Error> {
        self.child(v, self.key)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        mut self,
        _: &'static str,
        _: u32,
        v: &'static str,
        x: &T,
    ) -> Result<(), Self::Error> {
        self.token(22, v.as_bytes());
        self.child(x, false)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self, Self::Error> {
        self.out.push(23);
        Ok(self)
    }
    fn serialize_tuple(self, n: usize) -> Result<Self, Self::Error> {
        self.serialize_seq(Some(n))
    }
    fn serialize_tuple_struct(self, _: &'static str, n: usize) -> Result<Self, Self::Error> {
        self.serialize_seq(Some(n))
    }
    fn serialize_tuple_variant(
        mut self,
        _: &'static str,
        _: u32,
        v: &'static str,
        _: usize,
    ) -> Result<Self, Self::Error> {
        self.token(24, v.as_bytes());
        self.out.push(23);
        Ok(self)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self, Self::Error> {
        self.out.push(25);
        Ok(self)
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, Self::Error> {
        self.serialize_map(None)
    }
    fn serialize_struct_variant(
        mut self,
        _: &'static str,
        _: u32,
        v: &'static str,
        _: usize,
    ) -> Result<Self, Self::Error> {
        self.token(26, v.as_bytes());
        self.out.push(25);
        Ok(self)
    }
    fn collect_str<T: std::fmt::Display + ?Sized>(self, v: &T) -> Result<(), Self::Error> {
        // Custom Display serializers may encode floating signed zero; preserve
        // their complete output. Ordinary strings use serialize_str above.
        struct Text<'a>(&'a mut Vec<u8>);
        impl std::fmt::Write for Text<'_> {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                self.0.extend_from_slice(s.as_bytes());
                Ok(())
            }
        }
        self.out.push(27);
        let start = self.out.len();
        self.out.extend_from_slice(&0u64.to_le_bytes());
        write!(Text(&mut *self.out), "{v}").expect("Vec formatter");
        let len = (self.out.len() - start - 8) as u64;
        self.out[start..start + 8].copy_from_slice(&len.to_le_bytes());
        Ok(())
    }
}
macro_rules! sequence {($($trait:ident::$method:ident);*$(;)?)=>{$(impl $trait for Probe<'_>{type Ok=();type Error=serde_json::Error;fn $method<T:Serialize+?Sized>(&mut self,v:&T)->Result<(),Self::Error>{self.child(v,false)}fn end(self)->Result<(),Self::Error>{self.finish();Ok(())}})*}}
sequence! {SerializeSeq::serialize_element;SerializeTuple::serialize_element;SerializeTupleStruct::serialize_field;SerializeTupleVariant::serialize_field;}
impl SerializeMap for Probe<'_> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), Self::Error> {
        self.child(v, true)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), Self::Error> {
        self.child(v, false)
    }
    fn end(self) -> Result<(), Self::Error> {
        self.finish();
        Ok(())
    }
}
macro_rules! fields {($($trait:ident);*$(;)?)=>{$(impl $trait for Probe<'_>{type Ok=();type Error=serde_json::Error;fn serialize_field<T:Serialize+?Sized>(&mut self,k:&'static str,v:&T)->Result<(),Self::Error>{self.token(14,k.as_bytes());self.child(v,false)}fn end(self)->Result<(),Self::Error>{self.finish();Ok(())}})*}}
fields! {SerializeStruct;SerializeStructVariant;}
