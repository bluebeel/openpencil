//! Retain only compatibility metadata while validating the whole wire tree.
use super::probe::{Probe, Summary};
use serde::de::{DeserializeSeed, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::{collections::HashSet, fmt};
pub(super) struct Header {
    pub raw: Value,
    pub legacy: bool,
    pub legacy_doc_payload: bool,
}
impl<'de> Deserialize<'de> for Header {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct H;
        impl<'de> Visitor<'de> for H {
            type Value = Header;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a document map")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Header, A::Error> {
                let mut raw = Map::new();
                let mut seen = HashSet::new();
                let mut legacy = false;
                let mut pages_array = false;
                while let Some(key) = access.next_key::<String>()? {
                    legacy |= !seen.insert(key.clone());
                    let value = match key.as_str() {
                        "version" | "formatVersion" | "responsive" | "images" | "imageThumbs"
                        | "editorMeta" => {
                            let value: Value = access.next_value()?;
                            // Captured image/UI metadata can itself contain a
                            // legacy-shaped object; no clone/second DOM needed.
                            Probe(&mut legacy)
                                .deserialize(&value)
                                .map_err(serde::de::Error::custom)?;
                            value
                        }
                        _ => {
                            let summary = access.next_value_seed(Probe(&mut legacy))?;
                            if key == "pages" {
                                pages_array = matches!(summary, Summary::Array { .. })
                            }
                            Value::Null
                        }
                    };
                    raw.insert(key, value);
                }
                legacy |= raw.get("responsive").and_then(Value::as_bool) == Some(true);
                // The intermediate Figma writer needs authored child-order
                // repair. Explicitly supplied geometry flags always win.
                if let Some(meta) = raw.get("editorMeta").and_then(Value::as_object) {
                    legacy |= !meta.contains_key("preserveAuthoredGeometry")
                        && !meta.contains_key("preserve_authored_geometry");
                }
                let legacy_doc_payload =
                    pages_array && raw.get("version").is_some_and(|v| v.is_u64() || v.is_i64());
                Ok(Header {
                    raw: Value::Object(raw),
                    legacy,
                    legacy_doc_payload,
                })
            }
        }
        de.deserialize_map(H)
    }
}
