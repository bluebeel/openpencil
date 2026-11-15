//! Allocation-bounded structural probe for cases requiring the legacy DOM.
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::Deserializer;
use std::fmt;

#[derive(Clone, Copy)]
enum KeyKind {
    Other,
    Type,
    Id,
    Src,
    Fill,
    Thickness,
    Stroke,
    Corner,
    Spacing,
    Enabled,
    Geometry,
}
struct Key;
impl<'de> DeserializeSeed<'de> for Key {
    type Value = (u64, KeyKind);
    fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        struct K;
        impl<'de> Visitor<'de> for K {
            type Value = (u64, KeyKind);
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a JSON map key")
            }
            fn visit_str<E: serde::de::Error>(self, key: &str) -> Result<Self::Value, E> {
                let mut hash = 14695981039346656037_u64;
                for byte in key.bytes() {
                    hash = (hash ^ u64::from(byte)).wrapping_mul(1099511628211)
                }
                let kind = match key {
                    "type" => KeyKind::Type,
                    "id" => KeyKind::Id,
                    "src" => KeyKind::Src,
                    "fill" => KeyKind::Fill,
                    "thickness" => KeyKind::Thickness,
                    "stroke" => KeyKind::Stroke,
                    "cornerRadius" => KeyKind::Corner,
                    "padding" | "gap" | "margin" => KeyKind::Spacing,
                    "enabled" => KeyKind::Enabled,
                    "geometry" => KeyKind::Geometry,
                    _ => KeyKind::Other,
                };
                Ok((hash, kind))
            }
            fn visit_string<E: serde::de::Error>(self, key: String) -> Result<Self::Value, E> {
                self.visit_str(&key)
            }
        }
        de.deserialize_identifier(K)
    }
}
#[derive(Clone, Copy)]
pub(super) enum StringKind {
    Other,
    Ref,
    Image,
    Video,
    Path,
    LegacyNode,
    LegacyFill,
}
#[derive(Clone, Copy)]
pub(super) enum Summary {
    Scalar,
    String(StringKind),
    Object { legacy_fill: bool },
    Array { strings: bool, legacy_fills: bool },
}
impl Summary {
    fn has_string(self) -> bool {
        matches!(self, Self::String(_) | Self::Array { strings: true, .. })
    }
    fn legacy_fill(self) -> bool {
        matches!(
            self,
            Self::Object { legacy_fill: true }
                | Self::Array {
                    legacy_fills: true,
                    ..
                }
        )
    }
}
pub(super) struct Probe<'a>(pub &'a mut bool);
impl<'de> DeserializeSeed<'de> for Probe<'_> {
    type Value = Summary;
    fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<Summary, D::Error> {
        de.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Probe<'_> {
    type Value = Summary;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a valid JSON value")
    }
    fn visit_bool<E>(self, _: bool) -> Result<Summary, E> {
        Ok(Summary::Scalar)
    }
    fn visit_i64<E>(self, _: i64) -> Result<Summary, E> {
        Ok(Summary::Scalar)
    }
    fn visit_u64<E>(self, _: u64) -> Result<Summary, E> {
        Ok(Summary::Scalar)
    }
    fn visit_f64<E>(self, _: f64) -> Result<Summary, E> {
        Ok(Summary::Scalar)
    }
    fn visit_unit<E>(self) -> Result<Summary, E> {
        Ok(Summary::Scalar)
    }
    fn visit_none<E>(self) -> Result<Summary, E> {
        Ok(Summary::Scalar)
    }
    fn visit_str<E>(self, value: &str) -> Result<Summary, E> {
        let kind = match value {
            "video" => StringKind::Video,
            "image" => StringKind::Image,
            "path" => StringKind::Path,
            "icon" | "prompt" => StringKind::LegacyNode,
            "color" | "gradient" => StringKind::LegacyFill,
            v if v.starts_with('$') => StringKind::Ref,
            _ => StringKind::Other,
        };
        Ok(Summary::String(kind))
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Summary, E> {
        self.visit_str(&value)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Summary, A::Error> {
        let (mut strings, mut legacy_fills) = (false, false);
        while let Some(value) = access.next_element_seed(Probe(self.0))? {
            strings |= value.has_string();
            legacy_fills |= value.legacy_fill()
        }
        Ok(Summary::Array {
            strings,
            legacy_fills,
        })
    }
    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Summary, A::Error> {
        let mut hashes = [0_u64; 64];
        let mut len = 0;
        let mut overflow = Vec::new();
        let (mut has_type, mut has_id, mut has_src, mut stroke) = (false, false, false, false);
        let (mut fill_legacy, mut corner_ref, mut stroke_string) = (false, false, false);
        let mut geometry = false;
        let mut kind = StringKind::Other;
        while let Some((hash, key)) = access.next_key_seed(Key)? {
            // Collisions can only decline the fast path; equal keys always
            // decline, preserving the compatibility DOM's last-wins semantics.
            *self.0 |= hashes[..len].contains(&hash) || overflow.contains(&hash);
            if len < hashes.len() {
                hashes[len] = hash;
                len += 1
            } else {
                overflow.push(hash)
            }
            let value = access.next_value_seed(Probe(self.0))?;
            match key {
                KeyKind::Type => {
                    has_type = true;
                    if let Summary::String(v) = value {
                        kind = v
                    }
                }
                KeyKind::Id => has_id = true,
                KeyKind::Src => has_src = true,
                KeyKind::Thickness => stroke = true,
                KeyKind::Stroke => stroke_string = matches!(value, Summary::String(_)),
                KeyKind::Fill => {
                    fill_legacy = matches!(value, Summary::String(_) | Summary::Object { .. })
                        || value.legacy_fill()
                }
                KeyKind::Corner => corner_ref = value.has_string(),
                KeyKind::Spacing => {
                    *self.0 |= matches!(value, Summary::Array { strings: true, .. })
                }
                KeyKind::Enabled => {}
                KeyKind::Geometry => geometry = true,
                KeyKind::Other => {}
            }
        }
        let node = has_type && has_id;
        *self.0 |= matches!(kind, StringKind::Video)
            || (node
                && (matches!(kind, StringKind::LegacyNode)
                    || (matches!(kind, StringKind::Image) && !has_src)
                    || (matches!(kind, StringKind::Path) && geometry)
                    || corner_ref
                    || stroke_string))
            || ((node || stroke) && fill_legacy);
        Ok(Summary::Object {
            legacy_fill: matches!(kind, StringKind::LegacyFill),
        })
    }
}
