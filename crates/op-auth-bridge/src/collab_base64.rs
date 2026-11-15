//! Canonical decoding for bounded public keys, channel bindings and signatures.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;

pub(crate) struct SmallDecoded {
    bytes: [u8; 66],
    len: usize,
}

impl SmallDecoded {
    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// Keep the existing decode/re-encode equality check without a Vec or String.
/// The conservative66-byte decode buffer covers an86-character signature;
/// callers still enforce their own exact decoded lengths and error policy.
pub(crate) fn decode_small(value: &str) -> Option<SmallDecoded> {
    if value.len() > 86 || value.contains('=') {
        return None;
    }
    let mut bytes = [0; 66];
    let len = URL_SAFE_NO_PAD.decode_slice(value, &mut bytes).ok()?;
    let mut canonical = [0; 88];
    let encoded = URL_SAFE_NO_PAD
        .encode_slice(&bytes[..len], &mut canonical)
        .ok()?;
    if &canonical[..encoded] != value.as_bytes() {
        return None;
    }
    Some(SmallDecoded { bytes, len })
}
