//! Exact import-only vectors and colors; scalar field borrows materialize
//! a small read view lazily while numeric access stays allocation-free.
use super::FigKey;
use super::{DecodeScope, Decoder, DefKind, FigValue, KiwiError, Schema, MAX_DEPTH};
use std::sync::atomic::{AtomicPtr, Ordering};

pub struct FloatRecord<const N: usize> {
    values: [f32; N],
    // Pointer-sized, owned, immutable scalar view. A normal OnceLock adds
    // padding that would grow every FigValue beyond its 32-byte contract.
    views: AtomicPtr<[FigValue; N]>,
}
impl<const N: usize> FloatRecord<N> {
    pub(crate) fn new(values: [f32; N]) -> Self {
        Self {
            values,
            views: AtomicPtr::new(std::ptr::null_mut()),
        }
    }
    fn get(&self, key: &str, fields: &[&str; N]) -> Option<&FigValue> {
        let index = fields.iter().position(|name| *name == key)?;
        let mut pointer = self.views.load(Ordering::Acquire);
        if pointer.is_null() {
            let candidate = Box::into_raw(Box::new(self.values.map(FigValue::Float)));
            pointer = match self.views.compare_exchange(
                pointer,
                candidate,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => candidate,
                Err(existing) => {
                    // SAFETY: this thread's candidate was never published.
                    unsafe {
                        drop(Box::from_raw(candidate));
                    }
                    existing
                }
            };
        }
        // SAFETY: the published allocation is initialized before release and
        // immutable until exclusive mutation/drop. The returned borrow of self
        // prevents either operation while a scalar reference is alive.
        Some(unsafe { &(*pointer)[index] })
    }
    fn set(&mut self, key: &str, value: &FigValue, fields: &[&str; N]) -> bool {
        if let FigValue::Float(value) = value {
            if let Some(index) = fields.iter().position(|name| *name == key) {
                self.values[index] = *value;
                self.clear_views();
                return true;
            }
        }
        false
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        if self.views.load(Ordering::Acquire).is_null() {
            0
        } else {
            std::mem::size_of::<[FigValue; N]>()
        }
    }
    fn clear_views(&mut self) {
        let pointer = std::mem::replace(self.views.get_mut(), std::ptr::null_mut());
        if !pointer.is_null() {
            // SAFETY: exclusive access prevents readers and initialization.
            // The pointer owns exactly the Box published by get().
            unsafe {
                drop(Box::from_raw(pointer));
            }
        }
    }
    fn materialize(&self, fields: &[&str; N]) -> FigValue {
        FigValue::Object(
            fields
                .iter()
                .zip(self.values)
                .map(|(name, value)| ((*name).into(), FigValue::Float(value)))
                .collect(),
        )
    }
}

impl<const N: usize> Clone for FloatRecord<N> {
    fn clone(&self) -> Self {
        Self::new(self.values)
    }
}
impl<const N: usize> Drop for FloatRecord<N> {
    fn drop(&mut self) {
        self.clear_views();
    }
}
impl<const N: usize> std::fmt::Debug for FloatRecord<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("FloatRecord").field(&self.values).finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FloatKind {
    Vector,
    Color,
}
impl FloatKind {
    fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Vector => &["x", "y"],
            Self::Color => &["r", "g", "b", "a"],
        }
    }
    fn value(self, values: [f32; 4]) -> FigValue {
        match self {
            Self::Vector => FigValue::Vec2(FloatRecord::new([values[0], values[1]])),
            Self::Color => FigValue::Color(FloatRecord::new(values)),
        }
    }
}
pub(super) fn definitions(schema: &Schema) -> Vec<Option<(FloatKind, [u8; 4])>> {
    schema
        .definitions
        .iter()
        .map(|d| {
            if d.kind != DefKind::Struct
                || !d
                    .fields
                    .iter()
                    .all(|f| f.type_name == "float" && !f.is_array)
            {
                return None;
            }
            for kind in [FloatKind::Vector, FloatKind::Color] {
                let fields = kind.fields();
                if d.fields.len() != fields.len() {
                    continue;
                }
                let mut order = [0; 4];
                let mut seen = 0u8;
                let mut valid = true;
                for (i, field) in d.fields.iter().enumerate() {
                    let Some(index) = fields.iter().position(|name| *name == field.name) else {
                        valid = false;
                        break;
                    };
                    if seen & (1 << index) != 0 {
                        valid = false;
                        break;
                    }
                    seen |= 1 << index;
                    order[i] = index as u8;
                }
                if valid {
                    return Some((kind, order));
                }
            }
            None
        })
        .collect()
}
impl Decoder<'_, '_> {
    pub(super) fn decode_compact_float(
        &mut self,
        definition: usize,
        depth: u32,
        scope: DecodeScope,
    ) -> Result<Option<FigValue>, KiwiError> {
        if matches!(scope, DecodeScope::Full) {
            return Ok(None);
        }
        let Some((kind, order)) = self.compact_floats[definition] else {
            return Ok(None);
        };
        if self.schema.definitions[definition]
            .fields
            .iter()
            .any(|f| scope.field(&f.name).is_none())
        {
            return Ok(None);
        }
        if depth == MAX_DEPTH {
            return Err(KiwiError::TooDeep);
        }
        let mut values = [0.0; 4];
        for index in order.iter().take(kind.fields().len()) {
            values[*index as usize] = self.bb.read_var_float()?;
        }
        Ok(Some(kind.value(values)))
    }
}
impl FigValue {
    pub(super) fn small_float_get(&self, key: &str) -> Option<&FigValue> {
        match self {
            Self::Vec2(v) => v.get(key, &["x", "y"]),
            Self::Color(v) => v.get(key, &["r", "g", "b", "a"]),
            _ => None,
        }
    }
    pub(super) fn small_float_number(&self, key: &str) -> Option<f64> {
        let (kind, values) = match self {
            Self::Vec2(v) => (FloatKind::Vector, [v.values[0], v.values[1], 0.0, 0.0]),
            Self::Color(v) => (FloatKind::Color, v.values),
            _ => return None,
        };
        kind.fields()
            .iter()
            .position(|name| *name == key)
            .map(|i| f64::from(values[i]))
    }
    pub(crate) fn small_float_parts(&self) -> Option<(FloatKind, [f32; 4])> {
        match self {
            Self::Vec2(v) => Some((FloatKind::Vector, [v.values[0], v.values[1], 0.0, 0.0])),
            Self::Color(v) => Some((FloatKind::Color, v.values)),
            Self::Object(fields) => {
                for kind in [FloatKind::Vector, FloatKind::Color] {
                    let names = kind.fields();
                    if fields.len() != names.len() {
                        continue;
                    }
                    let mut values = [0.0; 4];
                    let mut seen = 0u8;
                    let mut valid = true;
                    for (name, value) in fields {
                        let Some(index) = names.iter().position(|key| *key == name.as_ref()) else {
                            valid = false;
                            break;
                        };
                        let Self::Float(value) = value else {
                            valid = false;
                            break;
                        };
                        if seen & (1 << index) != 0 {
                            valid = false;
                            break;
                        }
                        seen |= 1 << index;
                        values[index] = *value;
                    }
                    if valid {
                        return Some((kind, values));
                    }
                }
                None
            }
            _ => None,
        }
    }
    pub(super) fn set_small_float(&mut self, key: &str, value: &FigValue) -> bool {
        match self {
            Self::Vec2(v) => v.set(key, value, &["x", "y"]),
            Self::Color(v) => v.set(key, value, &["r", "g", "b", "a"]),
            _ => false,
        }
    }
    pub(super) fn materialize_small_float(&mut self) {
        match self {
            Self::Vec2(v) => *self = v.materialize(&["x", "y"]),
            Self::Color(v) => *self = v.materialize(&["r", "g", "b", "a"]),
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "compact_float_tests.rs"]
mod tests;

impl FigValue {
    /// Insert or overwrite a field on an object value (no-op for
    /// non-objects).
    pub fn set(&mut self, key: &str, value: FigValue) {
        if self.set_small_float(key, &value) || self.set_affine_number(key, &value) {
            return;
        }
        self.materialize_compact();
        if let FigValue::Object(pairs) = self {
            if let Some(slot) = pairs.iter_mut().find(|(k, _)| k.as_ref() == key) {
                slot.1 = value;
            } else {
                pairs.push((FigKey::from(key), value));
            }
        }
    }
}
