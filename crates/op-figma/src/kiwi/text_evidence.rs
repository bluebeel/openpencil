//! Import-specific streaming reduction of direct derived text caches.

use super::{DecodeScope, Decoder, DefKind, FigValue, KiwiError, NativeType, ResolvedType};
use crate::text_cache::evidence::{TextEvidence, NESTED_EVIDENCE};

impl Decoder<'_, '_> {
    pub(super) fn decode_text_evidence(
        &mut self,
        resolved: ResolvedType,
        depth: u32,
        nested: bool,
    ) -> Result<FigValue, KiwiError> {
        let ResolvedType::Definition(definition) = resolved else {
            return self.decode_type(resolved, depth, DecodeScope::Full);
        };
        let kind = self.schema.definitions[definition].kind;
        if kind == DefKind::Enum {
            return self.decode_enum(definition);
        }
        let mut evidence = TextEvidence::default();
        let mut next_struct_field = 0;
        loop {
            let field = if kind == DefKind::Struct {
                if next_struct_field == self.schema.definitions[definition].fields.len() {
                    break;
                }
                let index = next_struct_field;
                next_struct_field += 1;
                index
            } else {
                let id = self.bb.read_var_uint()?;
                if id == 0 {
                    break;
                }
                self.field_by_value[definition]
                    .get(&id)
                    .copied()
                    .ok_or_else(|| {
                        KiwiError::UnknownField(format!(
                            "{} id {id}",
                            self.schema.definitions[definition].name
                        ))
                    })?
            };
            self.decode_evidence_field(definition, field, depth, &mut evidence)?;
        }
        if nested {
            let mut fields = Vec::with_capacity(2);
            if let Some(characters) = &evidence.characters {
                fields.push(("characters".into(), FigValue::Str(characters.clone())));
            }
            fields.push((
                NESTED_EVIDENCE.into(),
                FigValue::TextEvidence(Box::new(evidence)),
            ));
            Ok(FigValue::Object(fields))
        } else {
            Ok(FigValue::TextEvidence(Box::new(evidence)))
        }
    }

    fn decode_evidence_field(
        &mut self,
        definition: usize,
        field: usize,
        depth: u32,
        evidence: &mut TextEvidence,
    ) -> Result<(), KiwiError> {
        let schema_field = &self.schema.definitions[definition].fields[field];
        let resolved = self.field_types[definition][field]
            .ok_or_else(|| KiwiError::BadType(schema_field.type_name.clone()))?;
        let name = schema_field.name.as_str();
        let is_array = schema_field.is_array;
        let Some(scope) = DecodeScope::DirectTextCache.field(name) else {
            return self.skip_field(resolved, is_array, depth);
        };
        if name == "characters" {
            evidence.characters = self
                .decode_field(resolved, is_array, depth, scope)?
                .as_str()
                .map(str::to_owned);
            return Ok(());
        }
        if name == "layoutSize" {
            evidence.observe_layout_size(&self.decode_field(resolved, is_array, depth, scope)?);
            return Ok(());
        }
        evidence.reset(name);
        if !is_array || resolved == ResolvedType::Native(NativeType::Byte) {
            return self.skip_field(resolved, is_array, depth);
        }
        let count = self.bb.read_var_uint()? as usize;
        if count > self.bb.remaining() {
            return Err(KiwiError::OutOfBounds);
        }
        // Each projected entry is consumed and released immediately. Array
        // lengths never reserve a retained FigValue or object-pair graph.
        match name {
            "glyphs" => {
                let mut consume = evidence.glyph_consumer();
                for _ in 0..count {
                    consume(&self.decode_type(resolved, depth + 1, scope)?);
                }
            }
            "baselines" => {
                for _ in 0..count {
                    evidence.observe_baseline(&self.decode_type(resolved, depth + 1, scope)?);
                }
            }
            "fontMetaData" => {
                for _ in 0..count {
                    evidence.observe_font(&self.decode_type(resolved, depth + 1, scope)?);
                }
            }
            _ => unreachable!("scope admits only text evidence arrays"),
        }
        Ok(())
    }
}
