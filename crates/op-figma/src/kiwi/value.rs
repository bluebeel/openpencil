//! Dynamic decoded values and representation-independent accessors.

use super::{storage, FigKey, FloatRecord};

/// Dynamically-typed decoded value — the Rust equivalent of the
/// untyped `any` tree the TS parser works with.
#[derive(Debug, Clone)]
pub enum FigValue {
    /// Absent / unresolved (e.g. an unknown enum ordinal).
    Null,
    Bool(bool),
    Int(i32),
    Uint(u32),
    Int64(i64),
    Uint64(u64),
    /// Import-only exact GUID struct; numeric access avoids a tiny heap object.
    Guid(u32, u32),
    /// Import-only exact 2×3 float transform, retaining all six authored values.
    Affine(storage::Affine),
    /// Import-only exact float structs, preserving borrowed scalar lookups.
    Vec2(FloatRecord<2>),
    Color(storage::Color),
    Float(f32),
    Str(storage::Text),
    Symbol(FigKey),
    Bytes(storage::Bytes),
    Array(storage::Values<FigValue>),
    /// Insertion-ordered key/value pairs (Kiwi struct/message). Keys decoded
    /// from the schema share their owned storage across object instances.
    Object(storage::Values<(FigKey, FigValue)>),
    /// Import-only, constant-space proof for linked text styles. Removed after
    /// style resolution; generic decoding never produces this variant.
    TextEvidence(Box<crate::text_cache::evidence::TextEvidence>),
    /// Compact authored line geometry retained only for Preserve-mode lists.
    TextLayout(Box<crate::text_cache::layout::SavedTextLayout>),
}

impl FigValue {
    /// Field lookup on an object value; `None` for non-objects, absent
    /// keys, or keys explicitly decoded to [`FigValue::Null`].
    pub fn get(&self, key: &str) -> Option<&FigValue> {
        match self {
            FigValue::Vec2(_) | FigValue::Color(_) => self.small_float_get(key),
            FigValue::Object(pairs) => pairs
                .iter()
                .find(|(k, _)| k.as_ref() == key)
                .map(|(_, v)| v)
                .filter(|v| !matches!(v, FigValue::Null)),
            _ => None,
        }
    }

    /// Mutable field lookup on an object value. Explicit `Null`
    /// remains absent, matching [`get`](Self::get).
    pub fn get_mut(&mut self, key: &str) -> Option<&mut FigValue> {
        self.materialize_compact();
        match self {
            FigValue::Object(pairs) => pairs
                .iter_mut()
                .find(|(name, _)| name.as_ref() == key)
                .map(|(_, value)| value)
                .filter(|value| !matches!(value, FigValue::Null)),
            _ => None,
        }
    }

    /// Numeric coercion across every integer/float variant.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            FigValue::Int(v) => Some(*v as f64),
            FigValue::Uint(v) => Some(*v as f64),
            FigValue::Int64(v) => Some(*v as f64),
            FigValue::Uint64(v) => Some(*v as f64),
            FigValue::Float(v) => Some(*v as f64),
            FigValue::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            FigValue::Str(s) => Some(s),
            FigValue::Symbol(s) => Some(s.as_ref()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            FigValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[FigValue]> {
        match self {
            FigValue::Array(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            FigValue::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn is_object(&self) -> bool {
        matches!(
            self,
            FigValue::Object(_)
                | FigValue::Guid(_, _)
                | FigValue::Affine(_)
                | FigValue::Vec2(_)
                | FigValue::Color(_)
        )
    }

    /// Numeric field read (`obj.key` coerced to f64).
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.compact_number(key)
            .or_else(|| self.get(key).and_then(|v| v.as_f64()))
    }

    /// String field read.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(|v| v.as_str())
    }

    /// Boolean field read.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(|v| v.as_bool())
    }

    /// Array field read.
    pub fn get_array(&self, key: &str) -> Option<&[FigValue]> {
        self.get(key).and_then(|v| v.as_array())
    }

    /// Mutable array field read.
    pub fn get_array_mut(&mut self, key: &str) -> Option<&mut storage::Values<FigValue>> {
        match self.get_mut(key)? {
            FigValue::Array(values) => Some(values),
            _ => None,
        }
    }
}
