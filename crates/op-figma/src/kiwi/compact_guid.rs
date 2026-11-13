//! Compact only the exact binary GUID struct on the import path. Generic
//! decoding retains its full object representation and schema field order.

use super::{DecodeScope, Decoder, DefKind, FigValue, KiwiError, Schema, MAX_DEPTH};

pub(super) fn definitions(schema: &Schema) -> Vec<Option<bool>> {
    schema
        .definitions
        .iter()
        .map(|definition| {
            let fields = &definition.fields;
            (definition.kind == DefKind::Struct
                && definition.name == "GUID"
                && fields.len() == 2
                && fields
                    .iter()
                    .all(|field| field.type_name == "uint" && !field.is_array)
                && ((fields[0].name == "sessionID" && fields[1].name == "localID")
                    || (fields[1].name == "sessionID" && fields[0].name == "localID")))
                .then(|| fields[0].name == "sessionID")
        })
        .collect()
}

impl Decoder<'_, '_> {
    pub(super) fn decode_compact_guid(
        &mut self,
        definition: usize,
        depth: u32,
        scope: DecodeScope,
    ) -> Result<Option<FigValue>, KiwiError> {
        if matches!(scope, DecodeScope::Full) {
            return Ok(None);
        }
        let Some(session_first) = self.compact_guids[definition] else {
            return Ok(None);
        };
        // The ordinary scalar fields each enter decode_type at depth+1.
        if depth == MAX_DEPTH {
            return Err(KiwiError::TooDeep);
        }
        let first = self.bb.read_var_uint()?;
        let second = self.bb.read_var_uint()?;
        Ok(Some(if session_first {
            FigValue::Guid(first, second)
        } else {
            FigValue::Guid(second, first)
        }))
    }
}

impl FigValue {
    pub(super) fn guid_number(&self, key: &str) -> Option<f64> {
        match (self, key) {
            (Self::Guid(session, _), "sessionID") => Some(f64::from(*session)),
            (Self::Guid(_, local), "localID") => Some(f64::from(*local)),
            _ => None,
        }
    }

    pub(crate) fn guid_parts(&self) -> Option<(u32, u32)> {
        match self {
            Self::Guid(session, local) => Some((*session, *local)),
            Self::Object(fields) if fields.len() == 2 => {
                let (Some(Self::Uint(session)), Some(Self::Uint(local))) =
                    (self.get("sessionID"), self.get("localID"))
                else {
                    return None;
                };
                Some((*session, *local))
            }
            _ => None,
        }
    }

    /// Rare mutable field access restores an ordinary object before borrowing.
    pub(super) fn materialize_guid(&mut self) {
        if let Self::Guid(session, local) = self {
            *self = Self::Object(vec![
                ("sessionID".into(), Self::Uint(*session)),
                ("localID".into(), Self::Uint(*local)),
            ]);
        }
    }
}

impl PartialEq for FigValue {
    fn eq(&self, other: &Self) -> bool {
        // Both ordinary GUID field orders normalize too: equality must remain
        // transitive when compact, generic and generated values are mixed.
        if let (Some(left), Some(right)) = (self.guid_parts(), other.guid_parts()) {
            return left == right;
        }
        if let (Some(left), Some(right)) = (self.affine_parts(), other.affine_parts()) {
            return left == right;
        }
        if let (Some(left), Some(right)) = (self.small_float_parts(), other.small_float_parts()) {
            return left == right;
        }
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Uint(a), Self::Uint(b)) => a == b,
            (Self::Int64(a), Self::Int64(b)) => a == b,
            (Self::Uint64(a), Self::Uint64(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => a == b,
            (Self::Str(a), Self::Str(b)) => a == b,
            (Self::Bytes(a), Self::Bytes(b)) => a == b,
            (Self::Array(a), Self::Array(b)) => a == b,
            (Self::Object(a), Self::Object(b)) => a == b,
            (Self::TextEvidence(a), Self::TextEvidence(b)) => a == b,
            (Self::TextLayout(a), Self::TextLayout(b)) => a == b,
            _ => false,
        }
    }
}

#[cfg(test)]
#[path = "compact_guid_tests.rs"]
mod tests;
