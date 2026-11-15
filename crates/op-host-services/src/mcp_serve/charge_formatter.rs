//! Count common geometry numbers without rendering their decimal digits.
//!
//! Only used with the resident ledger's byte counter, never a file writer.
//! The pinned JSON formatter prints integral f64 values below 1e16 in fixed
//! notation with `.0`. All other numbers retain its ordinary formatting.

use std::io::{self, Write};

use serde_json::ser::{CompactFormatter, Formatter};

pub(super) struct ChargeFormatter;

impl Formatter for ChargeFormatter {
    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        let magnitude = value.abs();
        if magnitude < 1e16 && value.fract() == 0.0 {
            let integer = magnitude as u64;
            let digits = if integer == 0 {
                1
            } else {
                integer.ilog10() as usize + 1
            };
            let count = digits + 2 + usize::from(value.is_sign_negative());
            // Contents are deliberately irrelevant: the sole writer counts
            // bytes. The maximum is 16 digits, `.0`, and a minus sign.
            writer.write_all(&b"0000000000000000000"[..count])
        } else {
            CompactFormatter.write_f64(writer, value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    fn counted<T: Serialize>(value: &T) -> usize {
        let mut bytes = Vec::new();
        value
            .serialize(&mut serde_json::Serializer::with_formatter(
                &mut bytes,
                ChargeFormatter,
            ))
            .unwrap();
        bytes.len()
    }

    #[test]
    fn geometry_number_counts_match_json_across_fixed_scientific_boundaries() {
        for value in [
            0.0,
            -0.0,
            1.0,
            -1.0,
            1e-5,
            1e-6,
            1.125,
            1e15,
            1e16,
            9_999_999_999_999_998.0,
            9_007_199_254_740_992.0,
            f64::MIN,
            f64::MAX,
            f64::MIN_POSITIVE,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ] {
            assert_eq!(
                counted(&value),
                serde_json::to_vec(&value).unwrap().len(),
                "{value:?}"
            );
        }
        let mut seed = 0x6d63705f636f756e_u64;
        for _ in 0..20_000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let value = f64::from_bits(seed);
            assert_eq!(
                counted(&value),
                serde_json::to_vec(&value).unwrap().len(),
                "bits={seed:x}"
            );
            let whole = (seed % 10_000_000_000_000_000) as f64;
            for value in [whole, -whole] {
                assert_eq!(
                    counted(&value),
                    serde_json::to_vec(&value).unwrap().len(),
                    "{value:?}"
                );
            }
        }
    }

    #[test]
    fn documents_keep_exact_string_container_and_non_numeric_counts() {
        let value = serde_json::json!({"escaped":"line\n\"quoted\"\\\u{0000}é",
            "geometry":[-0.0, 1920.0, 1080.0, 0.35], "nested":{"null":null,"flag":true}});
        assert_eq!(counted(&value), serde_json::to_vec(&value).unwrap().len());
    }
}
