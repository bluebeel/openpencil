//! Exact import-only affine packing. Generic decoding keeps ordinary objects;
//! extension fields and incompatible schema/field types keep the fallback.

use super::{DecodeScope, Decoder, DefKind, FigValue, KiwiError, Schema, MAX_DEPTH};

pub(super) const FIELDS: [&str; 6] = ["m00", "m01", "m02", "m10", "m11", "m12"];

pub(super) fn definitions(schema: &Schema) -> Vec<Option<[u8; 6]>> {
    schema
        .definitions
        .iter()
        .map(|definition| {
            if definition.kind != DefKind::Struct || definition.fields.len() != 6 {
                return None;
            }
            let mut order = [0; 6];
            let mut seen = 0u8;
            for (position, field) in definition.fields.iter().enumerate() {
                if field.type_name != "float" || field.is_array {
                    return None;
                }
                let index = FIELDS.iter().position(|name| *name == field.name)?;
                if seen & (1 << index) != 0 {
                    return None;
                }
                seen |= 1 << index;
                order[position] = index as u8;
            }
            Some(order)
        })
        .collect()
}

impl Decoder<'_, '_> {
    pub(super) fn decode_compact_record(
        &mut self,
        definition: usize,
        depth: u32,
        scope: DecodeScope,
    ) -> Result<Option<FigValue>, KiwiError> {
        if let Some(guid) = self.decode_compact_guid(definition, depth, scope)? {
            return Ok(Some(guid));
        }
        if let Some(affine) = self.decode_compact_affine(definition, depth, scope)? {
            return Ok(Some(affine));
        }
        self.decode_compact_float(definition, depth, scope)
    }

    fn decode_compact_affine(
        &mut self,
        definition: usize,
        depth: u32,
        scope: DecodeScope,
    ) -> Result<Option<FigValue>, KiwiError> {
        if matches!(scope, DecodeScope::Full) {
            return Ok(None);
        }
        let Some(order) = self.compact_affines[definition] else {
            return Ok(None);
        };
        // Cache projections may deliberately skip these fields. Preserve the
        // ordinary validated skip rather than bypassing that projection.
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
        let mut values = [0.0; 6];
        for index in order {
            values[index as usize] = self.bb.read_var_float()?;
        }
        Ok(Some(FigValue::Affine(values.into())))
    }
}

impl FigValue {
    pub(super) fn compact_number(&self, key: &str) -> Option<f64> {
        self.small_float_number(key)
            .or_else(|| self.guid_number(key))
            .or_else(|| {
                let Self::Affine(values) = self else {
                    return None;
                };
                FIELDS
                    .iter()
                    .position(|name| *name == key)
                    .map(|index| f64::from(values[index]))
            })
    }

    pub(crate) fn affine_parts(&self) -> Option<[f32; 6]> {
        match self {
            Self::Affine(values) => {
                let mut copy = [0.0; 6];
                copy.copy_from_slice(&values[..]);
                Some(copy)
            }
            Self::Object(fields) if fields.len() == 6 => {
                let mut values = [0.0; 6];
                let mut seen = 0u8;
                for (name, value) in fields {
                    let index = FIELDS.iter().position(|key| *key == name.as_ref())?;
                    let Self::Float(value) = value else {
                        return None;
                    };
                    if seen & (1 << index) != 0 {
                        return None;
                    }
                    seen |= 1 << index;
                    values[index] = *value;
                }
                Some(values)
            }
            _ => None,
        }
    }

    /// Scaling translations is common: keep existing float fields inline.
    /// Other value types and extension fields materialize the complete object.
    pub(super) fn set_affine_number(&mut self, key: &str, value: &FigValue) -> bool {
        if let (Self::Affine(values), Self::Float(value)) = (self, value) {
            if let Some(index) = FIELDS.iter().position(|name| *name == key) {
                values[index] = *value;
                return true;
            }
        }
        false
    }

    pub(super) fn materialize_compact(&mut self) {
        self.materialize_guid();
        self.materialize_small_float();
        if let Self::Affine(values) = self {
            *self = Self::Object(
                FIELDS
                    .iter()
                    .zip(values.iter())
                    .map(|(name, value)| ((*name).into(), Self::Float(*value)))
                    .collect(),
            );
        }
    }
}

#[cfg(test)]
#[path = "compact_affine_tests.rs"]
mod tests;
