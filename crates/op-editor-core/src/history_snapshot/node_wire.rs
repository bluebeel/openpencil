//! Forward the canonical serializer, replacing only this node's child value.
//! Field order/skip rules come from the schema, including future metadata fields.
use super::{SharedNode, SharedNodes};
use serde::{
    ser::{SerializeMap, SerializeStruct},
    Serialize, Serializer,
};
use std::{marker::PhantomData, sync::Arc};
pub(super) struct ReplaceChildren<'a, S> {
    inner: S,
    children: Option<&'a [Arc<SharedNode>]>,
    omit_children: bool,
}
impl<'a, S> ReplaceChildren<'a, S> {
    pub fn new(inner: S, children: Option<&'a [Arc<SharedNode>]>) -> Self {
        Self {
            inner,
            children,
            omit_children: false,
        }
    }
    pub fn omitting_children(inner: S) -> Self {
        Self {
            inner,
            children: None,
            omit_children: true,
        }
    }
}
pub(super) struct Map<'a, M> {
    inner: M,
    children: Option<&'a [Arc<SharedNode>]>,
    omit_children: bool,
    pending: bool,
}
pub(super) struct Struct<'a, M> {
    inner: M,
    children: Option<&'a [Arc<SharedNode>]>,
    omit_children: bool,
}
macro_rules! forward {($($name:ident($($arg:ident:$ty:ty),*));*$(;)?)=>{$(fn $name(self,$($arg:$ty),*)->Result<Self::Ok,Self::Error>{self.inner.$name($($arg),*)})*}}
impl<'a, S: Serializer> Serializer for ReplaceChildren<'a, S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = S::SerializeSeq;
    type SerializeTuple = S::SerializeTuple;
    type SerializeTupleStruct = S::SerializeTupleStruct;
    type SerializeTupleVariant = S::SerializeTupleVariant;
    type SerializeMap = Map<'a, S::SerializeMap>;
    type SerializeStruct = Struct<'a, S::SerializeStruct>;
    type SerializeStructVariant = S::SerializeStructVariant;
    forward! {serialize_bool(v:bool);serialize_i8(v:i8);serialize_i16(v:i16);serialize_i32(v:i32);serialize_i64(v:i64);serialize_i128(v:i128);serialize_u8(v:u8);serialize_u16(v:u16);serialize_u32(v:u32);serialize_u64(v:u64);serialize_u128(v:u128);serialize_f32(v:f32);serialize_f64(v:f64);serialize_char(v:char);serialize_str(v:&str);serialize_bytes(v:&[u8]);serialize_none();serialize_unit();serialize_unit_struct(name:&'static str);serialize_unit_variant(name:&'static str,index:u32,variant:&'static str);}
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_some(v)
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        n: &'static str,
        v: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_newtype_struct(n, v)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        n: &'static str,
        i: u32,
        v: &'static str,
        x: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_newtype_variant(n, i, v, x)
    }
    fn serialize_seq(self, n: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.inner.serialize_seq(n)
    }
    fn serialize_tuple(self, n: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.inner.serialize_tuple(n)
    }
    fn serialize_tuple_struct(
        self,
        n: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.inner.serialize_tuple_struct(n, len)
    }
    fn serialize_tuple_variant(
        self,
        n: &'static str,
        i: u32,
        v: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.inner.serialize_tuple_variant(n, i, v, len)
    }
    fn serialize_map(self, n: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Ok(Map {
            inner: self.inner.serialize_map(n)?,
            children: self.children,
            omit_children: self.omit_children,
            pending: false,
        })
    }
    fn serialize_struct(
        self,
        n: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(Struct {
            inner: self.inner.serialize_struct(n, len)?,
            children: self.children,
            omit_children: self.omit_children,
        })
    }
    fn serialize_struct_variant(
        self,
        n: &'static str,
        i: u32,
        v: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.inner.serialize_struct_variant(n, i, v, len)
    }
    fn collect_str<T: ?Sized + std::fmt::Display>(self, v: &T) -> Result<Self::Ok, Self::Error> {
        self.inner.collect_str(v)
    }
    fn is_human_readable(&self) -> bool {
        self.inner.is_human_readable()
    }
}
impl<M: SerializeMap> SerializeMap for Map<'_, M> {
    type Ok = M::Ok;
    type Error = M::Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Self::Error> {
        self.pending = key.serialize(Key::<Self::Error>(PhantomData))?;
        if self.pending && self.omit_children {
            return Ok(());
        }
        self.inner.serialize_key(key)
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        if std::mem::take(&mut self.pending) {
            if self.omit_children {
                return Ok(());
            }
            let c = self
                .children
                .ok_or_else(|| serde::ser::Error::custom("missing captured children"))?;
            self.inner.serialize_value(&SharedNodes(c))
        } else {
            self.inner.serialize_value(value)
        }
    }
    fn serialize_entry<K: ?Sized + Serialize, V: ?Sized + Serialize>(
        &mut self,
        key: &K,
        value: &V,
    ) -> Result<(), Self::Error> {
        if key.serialize(Key::<Self::Error>(PhantomData))? {
            if self.omit_children {
                return Ok(());
            }
            let c = self
                .children
                .ok_or_else(|| serde::ser::Error::custom("missing captured children"))?;
            self.inner.serialize_entry(key, &SharedNodes(c))
        } else {
            self.inner.serialize_entry(key, value)
        }
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.inner.end()
    }
}
impl<M: SerializeStruct> SerializeStruct for Struct<'_, M> {
    type Ok = M::Ok;
    type Error = M::Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        if key == "children" {
            if self.omit_children {
                return Ok(());
            }
            let c = self
                .children
                .ok_or_else(|| serde::ser::Error::custom("missing captured children"))?;
            self.inner.serialize_field(key, &SharedNodes(c))
        } else {
            self.inner.serialize_field(key, value)
        }
    }
    fn skip_field(&mut self, k: &'static str) -> Result<(), Self::Error> {
        self.inner.skip_field(k)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.inner.end()
    }
}
// Canonical field keys serialize as strings. Probe without allocating key JSON.
struct Key<E>(PhantomData<E>);
macro_rules! no_key {($($name:ident($($a:ident:$ty:ty),*));*$(;)?)=>{$(fn $name(self,$($a:$ty),*)->Result<bool,E>{$(let _=$a;)*Ok(false)})*}}
impl<E: serde::ser::Error> Serializer for Key<E> {
    type Ok = bool;
    type Error = E;
    type SerializeSeq = serde::ser::Impossible<bool, E>;
    type SerializeTuple = Self::SerializeSeq;
    type SerializeTupleStruct = Self::SerializeSeq;
    type SerializeTupleVariant = Self::SerializeSeq;
    type SerializeMap = Self::SerializeSeq;
    type SerializeStruct = Self::SerializeSeq;
    type SerializeStructVariant = Self::SerializeSeq;
    no_key! {serialize_bool(v:bool);serialize_i8(v:i8);serialize_i16(v:i16);serialize_i32(v:i32);serialize_i64(v:i64);serialize_u8(v:u8);serialize_u16(v:u16);serialize_u32(v:u32);serialize_u64(v:u64);serialize_f32(v:f32);serialize_f64(v:f64);serialize_char(v:char);serialize_bytes(v:&[u8]);serialize_none();serialize_unit();serialize_unit_struct(n:&'static str);serialize_unit_variant(n:&'static str,i:u32,v:&'static str);}
    fn serialize_str(self, v: &str) -> Result<bool, E> {
        Ok(v == "children")
    }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<bool, E> {
        v.serialize(self)
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        v: &T,
    ) -> Result<bool, E> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<bool, E> {
        Ok(false)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, E> {
        Err(E::custom("non-scalar schema field key"))
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, E> {
        Err(E::custom("non-scalar schema field key"))
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, E> {
        Err(E::custom("non-scalar schema field key"))
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, E> {
        Err(E::custom("non-scalar schema field key"))
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, E> {
        Err(E::custom("non-scalar schema field key"))
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeStruct, E> {
        Err(E::custom("non-scalar schema field key"))
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, E> {
        Err(E::custom("non-scalar schema field key"))
    }
    fn collect_str<T: ?Sized + std::fmt::Display>(self, v: &T) -> Result<bool, E> {
        Ok(v.to_string() == "children")
    }
}
