//! Validate without building Value maps/arrays or owning decoded strings.
//! Use deserialize_any, not IgnoredAny: the ordinary decoder must still check
//! floating-point range and Unicode/surrogate escapes in every value/key.
use serde::{de, Deserialize, Deserializer};
use std::fmt;

pub(super) fn object(input: &str) -> bool {
    input.trim_start().starts_with('{') && serde_json::from_str::<Validated>(input).is_ok()
}

struct Validated;
impl<'de> Deserialize<'de> for Validated {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor)
    }
}
struct ValueVisitor;
impl<'de> de::Visitor<'de> for ValueVisitor {
    type Value = Validated;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a valid JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Validated, E> {
        Ok(Validated)
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Validated, E> {
        Ok(Validated)
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Validated, E> {
        Ok(Validated)
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Validated, E> {
        Ok(Validated)
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Validated, E> {
        if value.is_finite() {
            Ok(Validated)
        } else {
            Err(E::custom("JSON number is not finite"))
        }
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<Validated, E> {
        Ok(Validated)
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, mut sequence: A) -> Result<Validated, A::Error> {
        while sequence.next_element::<Validated>()?.is_some() {}
        Ok(Validated)
    }

    fn visit_map<A: de::MapAccess<'de>>(self, mut object: A) -> Result<Validated, A::Error> {
        while object.next_entry::<Validated, Validated>()?.is_some() {}
        Ok(Validated)
    }
}
