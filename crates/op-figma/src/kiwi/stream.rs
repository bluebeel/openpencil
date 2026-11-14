//! Schema-framed import. Retains one exact node/blob wire value at a time.
use super::*;
use std::io::BufRead;

struct Scanner<'a, R> {
    reader: R,
    left: usize,
    template: Decoder<'a, 'static>,
    scratch: Vec<u8>,
    packed: bool,
    blobs: Option<Vec<crate::figma_types::BlobOrString>>,
}
impl<'a, R: BufRead> Scanner<'a, R> {
    fn byte(&mut self, out: &mut Vec<u8>) -> Result<u8, KiwiError> {
        if self.left == 0 {
            return Err(KiwiError::OutOfBounds);
        }
        let mut byte = [0];
        self.reader.read_exact(&mut byte).map_err(io)?;
        self.left -= 1;
        out.push(byte[0]);
        Ok(byte[0])
    }
    fn uint(&mut self, out: &mut Vec<u8>) -> Result<u32, KiwiError> {
        let mut value = 0u32;
        let mut shift = 0;
        loop {
            let b = self.byte(out)?;
            value |= ((b & 127) as u32).wrapping_shl(shift);
            shift += 7;
            if b & 128 == 0 || shift >= 35 {
                return Ok(value);
            }
        }
    }
    fn bytes(&mut self, count: usize, out: &mut Vec<u8>) -> Result<(), KiwiError> {
        if count > self.left {
            return Err(KiwiError::OutOfBounds);
        }
        let start = out.len();
        out.resize(start + count, 0);
        self.reader.read_exact(&mut out[start..]).map_err(io)?;
        self.left -= count;
        Ok(())
    }
    fn string(&mut self, out: &mut Vec<u8>) -> Result<(), KiwiError> {
        loop {
            if self.left == 0 {
                return Err(KiwiError::OutOfBounds);
            }
            let data = self.reader.fill_buf().map_err(io)?;
            if data.is_empty() {
                return Err(KiwiError::OutOfBounds);
            }
            let available = &data[..data.len().min(self.left)];
            let end = available.iter().position(|b| *b == 0);
            let count = end.map_or(available.len(), |i| i + 1);
            out.extend_from_slice(&available[..count]);
            self.reader.consume(count);
            self.left -= count;
            if end.is_some() {
                return Ok(());
            }
        }
    }
    fn field(
        &mut self,
        kind: ResolvedType,
        array: bool,
        depth: u32,
        out: &mut Vec<u8>,
    ) -> Result<(), KiwiError> {
        if !array {
            return self.value(kind, depth + 1, out);
        }
        let count = self.uint(out)? as usize;
        if count > self.left {
            return Err(KiwiError::OutOfBounds);
        }
        if kind == ResolvedType::Native(NativeType::Byte) {
            return self.bytes(count, out);
        }
        for _ in 0..count {
            self.value(kind, depth + 1, out)?
        }
        Ok(())
    }
    fn value(
        &mut self,
        kind: ResolvedType,
        depth: u32,
        out: &mut Vec<u8>,
    ) -> Result<(), KiwiError> {
        if depth > MAX_DEPTH {
            return Err(KiwiError::TooDeep);
        }
        match kind {
            ResolvedType::Native(native) => match native {
                NativeType::Bool | NativeType::Byte => {
                    self.byte(out)?;
                }
                NativeType::Int | NativeType::Uint => {
                    self.uint(out)?;
                }
                NativeType::Float => {
                    if self.byte(out)? != 0 {
                        self.bytes(3, out)?
                    }
                }
                NativeType::String => self.string(out)?,
                NativeType::Int64 | NativeType::Uint64 => {
                    for group in 0..9 {
                        let b = self.byte(out)?;
                        if group == 8 || b & 128 == 0 {
                            break;
                        }
                    }
                }
            },
            ResolvedType::Definition(definition) => {
                match self.template.schema.definitions[definition].kind {
                    DefKind::Enum => {
                        self.uint(out)?;
                    }
                    DefKind::Struct => {
                        for index in 0..self.template.schema.definitions[definition].fields.len() {
                            self.schema_field(definition, index, depth, out)?
                        }
                    }
                    DefKind::Message => loop {
                        let id = self.uint(out)?;
                        if id == 0 {
                            break;
                        }
                        let index = self.index(definition, id)?;
                        self.schema_field(definition, index, depth, out)?
                    },
                }
            }
        }
        Ok(())
    }
    fn index(&self, definition: usize, id: u32) -> Result<usize, KiwiError> {
        self.template.field_by_value[definition]
            .get(&id)
            .copied()
            .ok_or_else(|| {
                KiwiError::UnknownField(format!(
                    "{} id {id}",
                    self.template.schema.definitions[definition].name
                ))
            })
    }
    fn schema_field(
        &mut self,
        definition: usize,
        index: usize,
        depth: u32,
        out: &mut Vec<u8>,
    ) -> Result<(), KiwiError> {
        let field = &self.template.schema.definitions[definition].fields[index];
        let array = field.is_array;
        let kind = self.template.field_types[definition][index]
            .ok_or_else(|| KiwiError::BadType(field.type_name.clone()))?;
        self.field(kind, array, depth, out)
    }
    fn decode_value(
        &mut self,
        kind: ResolvedType,
        depth: u32,
        scope: DecodeScope,
    ) -> Result<FigValue, KiwiError> {
        let mut bytes = std::mem::take(&mut self.scratch);
        bytes.clear();
        self.value(kind, depth, &mut bytes)?;
        let mut decoder = Decoder {
            schema: self.template.schema,
            bb: ByteBuffer::with_tail(&bytes, self.left),
            field_types: self.template.field_types.clone(),
            field_by_value: self.template.field_by_value.clone(),
            object_keys: self.template.object_keys.clone(),
            compact_guids: self.template.compact_guids.clone(),
            compact_affines: self.template.compact_affines.clone(),
            compact_floats: self.template.compact_floats.clone(),
            lazy_context: self.template.lazy_context.clone(),
        };
        let value = decoder.decode_type(kind, depth, scope)?;
        assert_eq!(decoder.bb.remaining(), 0);
        self.template.lazy_context = decoder.lazy_context;
        bytes.clear();
        self.scratch = bytes;
        Ok(value)
    }
    fn decode_field(
        &mut self,
        kind: ResolvedType,
        array: bool,
        depth: u32,
        scope: DecodeScope,
        blob_array: bool,
    ) -> Result<FigValue, KiwiError> {
        if !array {
            return self.decode_value(kind, depth + 1, scope);
        }
        let count = self.uint(&mut Vec::new())? as usize;
        if count > self.left {
            return Err(KiwiError::OutOfBounds);
        }
        if kind == ResolvedType::Native(NativeType::Byte) {
            let mut bytes = Vec::new();
            self.bytes(count, &mut bytes)?;
            return Ok(FigValue::Bytes(bytes.into()));
        }
        let mut values = Vec::with_capacity(if blob_array { 0 } else { count });
        let mut packed = Vec::with_capacity(if blob_array { count } else { 0 });
        for _ in 0..count {
            let value = self.decode_value(kind, depth + 1, scope)?;
            if blob_array {
                packed.push(crate::figma_types::extract_blob(value).packed())
            } else {
                values.push(value)
            }
        }
        if blob_array {
            self.blobs = Some(packed)
        }
        Ok(FigValue::Array(values.into()))
    }
    fn root(&mut self, definition: usize) -> Result<FigValue, KiwiError> {
        let mut pairs: Vec<(FigKey, FigValue)> = Vec::new();
        let mut slots: Vec<Option<usize>> =
            vec![None; self.template.schema.definitions[definition].fields.len()];
        let mut blob_slots = std::collections::BTreeMap::new();
        loop {
            let id = self.uint(&mut Vec::new())?;
            if id == 0 {
                break;
            }
            let index = self.index(definition, id)?;
            let field = &self.template.schema.definitions[definition].fields[index];
            let array = field.is_array;
            let kind = self.template.field_types[definition][index]
                .ok_or_else(|| KiwiError::BadType(field.type_name.clone()))?;
            let name = self.template.object_keys[definition][index].clone();
            let scope = DecodeScope::Root.field(&name).unwrap();
            let value = self.decode_field(
                kind,
                array,
                0,
                scope,
                self.packed && name.as_ref() == "blobs" && array,
            )?;
            let slot = if let Some(slot) = slots[index] {
                pairs[slot].1 = value;
                slot
            } else {
                let slot = pairs.len();
                slots[index] = Some(slot);
                pairs.push((name, value));
                slot
            };
            if let Some(blobs) = self.blobs.take() {
                blob_slots.insert(slot, blobs);
            }
        }
        if self.left != 0 {
            return Err(KiwiError::TrailingBytes(self.left));
        }
        self.blobs = blob_slots.into_iter().next_back().map(|(_, v)| v);
        Ok(FigValue::Object(pairs.into()))
    }
}
fn io(error: std::io::Error) -> KiwiError {
    if error.kind() == std::io::ErrorKind::UnexpectedEof {
        KiwiError::OutOfBounds
    } else {
        KiwiError::BadType(format!("stream read: {error}"))
    }
}
#[cfg(test)]
pub(crate) fn decode<R: BufRead>(
    schema: &Schema,
    reader: R,
    len: usize,
) -> Result<FigValue, KiwiError> {
    decode_with_blobs(schema, reader, len, false).map(|v| v.0)
}
pub(crate) fn decode_with_blobs<R: BufRead>(
    schema: &Schema,
    reader: R,
    len: usize,
    packed: bool,
) -> Result<(FigValue, Option<Vec<crate::figma_types::BlobOrString>>), KiwiError> {
    let root = schema
        .definitions
        .iter()
        .position(|d| d.name == "Message" && d.kind == DefKind::Message)
        .or_else(|| {
            schema
                .definitions
                .iter()
                .position(|d| d.kind == DefKind::Message)
        })
        .ok_or(KiwiError::NoRootMessage)?;
    let template = Decoder::new(schema, &[])?;
    let mut scanner = Scanner {
        reader,
        left: len,
        template,
        scratch: Vec::new(),
        packed,
        blobs: None,
    };
    let root = scanner.root(root)?;
    Ok((root, scanner.blobs))
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
