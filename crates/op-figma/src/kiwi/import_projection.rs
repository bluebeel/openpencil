//! Import-only cache projection and allocation-free validated wire skipping.

use super::{Decoder, DefKind, KiwiError, NativeType, ResolvedType, MAX_DEPTH};

#[derive(Clone, Copy)]
pub(super) enum DecodeScope {
    Full,
    Import,
    Root,
    DirectNode,
    DirectTextCache,
    GlyphEvidence,
    PointEvidence,
    BaselineEvidence,
    FontMetadataEvidence,
    FontKeyEvidence,
    Instance,
    NestedTextCache,
    Presence,
}

impl DecodeScope {
    pub(super) fn retained_fields(self, keys: &[std::sync::Arc<str>]) -> usize {
        if matches!(self, Self::Full) {
            keys.len()
        } else {
            keys.iter()
                .filter(|name| self.field(name).is_some())
                .count()
        }
    }

    pub(super) fn compact_object(self) -> bool {
        matches!(
            self,
            Self::DirectTextCache
                | Self::GlyphEvidence
                | Self::PointEvidence
                | Self::BaselineEvidence
                | Self::FontMetadataEvidence
                | Self::FontKeyEvidence
                | Self::NestedTextCache
        )
    }

    pub(super) fn exact_message_capacity(self) -> bool {
        matches!(
            self,
            Self::Root | Self::Import | Self::DirectNode | Self::Instance
        )
    }

    pub(super) fn field(self, name: &str) -> Option<Self> {
        use DecodeScope::*;
        match (self, name) {
            (Root, "nodeChanges") => Some(DirectNode),
            (Root | Import, _) => Some(Import),
            (DirectNode | Instance, "parameterConsumptionMap" | "variableConsumptionMap") => {
                Some(Presence)
            }
            (DirectNode | Instance, "legacyDerivedTextData") => None,
            (DirectNode, "derivedTextData") => Some(DirectTextCache),
            (DirectNode, "symbolData" | "derivedSymbolData") => Some(Instance),
            (DirectTextCache, "characters") => Some(Full),
            (DirectTextCache, "layoutSize") => Some(PointEvidence),
            (DirectTextCache, "glyphs") => Some(GlyphEvidence),
            (DirectTextCache, "baselines") => Some(BaselineEvidence),
            (DirectTextCache, "fontMetaData") => Some(FontMetadataEvidence),
            (DirectTextCache, _) => None,
            (GlyphEvidence, "position") => Some(PointEvidence),
            (GlyphEvidence, "fontSize" | "advance" | "firstCharacter" | "rotation") => Some(Full),
            (GlyphEvidence, _) => None,
            (PointEvidence, "x" | "y") => Some(Full),
            (PointEvidence, _) => None,
            (BaselineEvidence, "position") => Some(PointEvidence),
            (BaselineEvidence, "lineHeight" | "lineY" | "firstCharacter" | "endCharacter") => {
                Some(Full)
            }
            (BaselineEvidence, _) => None,
            (FontMetadataEvidence, "key") => Some(FontKeyEvidence),
            (FontMetadataEvidence, _) => None,
            (FontKeyEvidence, "family" | "style") => Some(Full),
            (FontKeyEvidence, _) => None,
            (Instance, "derivedTextData") => Some(NestedTextCache),
            (Instance, _) => Some(Instance),
            (DirectNode, _) => Some(Import),
            (NestedTextCache, "characters") => Some(Full),
            (NestedTextCache, _) => None,
            (Presence, _) => None,
            _ => Some(Full),
        }
    }
}

impl Decoder<'_, '_> {
    /// Count actual retained message fields with the same validated wire walk,
    /// then rewind. Avoid both schema-wide reservation on sparse NodeChange
    /// and geometric Vec over-allocation across millions of small messages.
    pub(super) fn import_message_capacity(
        &mut self,
        definition: usize,
        depth: u32,
        scope: DecodeScope,
        slots: &mut [Option<usize>],
    ) -> Result<usize, KiwiError> {
        let start = self.bb.index;
        let mut count = 0;
        loop {
            let id = self.bb.read_var_uint()?;
            if id == 0 {
                break;
            }
            let field = self.field_by_value[definition]
                .get(&id)
                .copied()
                .ok_or_else(|| {
                    KiwiError::UnknownField(format!(
                        "{} id {id}",
                        self.schema.definitions[definition].name
                    ))
                })?;
            if slots[field].is_none() && scope.field(&self.object_keys[definition][field]).is_some()
            {
                slots[field] = Some(count);
                count += 1;
            }
            self.skip_schema_field(definition, field, depth)?;
        }
        self.bb.index = start;
        slots.fill(None);
        Ok(count)
    }

    pub(super) fn skip_field(
        &mut self,
        resolved: ResolvedType,
        is_array: bool,
        depth: u32,
    ) -> Result<(), KiwiError> {
        if !is_array {
            return self.skip_type(resolved, depth + 1);
        }
        if resolved == ResolvedType::Native(NativeType::Byte) {
            return self.bb.skip_byte_array();
        }
        let count = self.bb.read_var_uint()? as usize;
        if count > self.bb.remaining() {
            return Err(KiwiError::OutOfBounds);
        }
        for _ in 0..count {
            self.skip_type(resolved, depth + 1)?;
        }
        Ok(())
    }

    fn skip_type(&mut self, resolved: ResolvedType, depth: u32) -> Result<(), KiwiError> {
        if depth > MAX_DEPTH {
            return Err(KiwiError::TooDeep);
        }
        match resolved {
            ResolvedType::Native(native) => match native {
                NativeType::Bool | NativeType::Byte => {
                    self.bb.read_byte()?;
                }
                NativeType::Int | NativeType::Uint => {
                    self.bb.read_var_uint()?;
                }
                NativeType::Float => {
                    self.bb.read_var_float()?;
                }
                NativeType::String => self.bb.skip_string()?,
                NativeType::Int64 | NativeType::Uint64 => {
                    self.bb.read_var_uint64()?;
                }
            },
            ResolvedType::Definition(index) => match self.schema.definitions[index].kind {
                DefKind::Enum => {
                    self.bb.read_var_uint()?;
                }
                DefKind::Struct => {
                    for field in 0..self.schema.definitions[index].fields.len() {
                        self.skip_schema_field(index, field, depth)?;
                    }
                }
                DefKind::Message => loop {
                    let id = self.bb.read_var_uint()?;
                    if id == 0 {
                        break;
                    }
                    let field = self.field_by_value[index]
                        .get(&id)
                        .copied()
                        .ok_or_else(|| {
                            KiwiError::UnknownField(format!(
                                "{} id {id}",
                                self.schema.definitions[index].name
                            ))
                        })?;
                    self.skip_schema_field(index, field, depth)?;
                },
            },
        }
        Ok(())
    }

    fn skip_schema_field(
        &mut self,
        definition: usize,
        field: usize,
        depth: u32,
    ) -> Result<(), KiwiError> {
        let schema_field = &self.schema.definitions[definition].fields[field];
        let resolved = self.field_types[definition][field]
            .ok_or_else(|| KiwiError::BadType(schema_field.type_name.clone()))?;
        self.skip_field(resolved, schema_field.is_array, depth)
    }
}
