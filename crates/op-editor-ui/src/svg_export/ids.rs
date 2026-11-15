//! Injective scene IDs in namespaces disjoint from generated paint resources.

use super::svg_id;

// Keep ordinary authored IDs stable. A canonical ID that resembles one of
// our generated resources is escaped into the reserved node namespace too,
// so a real node named `gradient-n-61` cannot shadow node `a`'s gradient.
const RESERVED: &[&str] = &[
    "image-field-",
    "image-source-",
    "image-tile-",
    "image-cell-",
    "image-viewport-",
    "image-unit-",
    "image-repeat-",
    "image-edge-",
    "image-adjust-",
    "fill-paint-",
    "node-n-",
    "gradient-n-",
    "clip-n-",
    "ancestor-clip-n-",
    "image-pattern-n-",
    "image-pattern-stroke-paint-",
    "mask-n-",
    "vector-mask-n-",
    "ancestor-mask-n-",
    "ancestor-vector-mask-n-",
    "visual-clip-n-",
    "stroke-interior-",
    "stroke-exterior-",
    "stroke-coverage-",
    "stroke-sides-",
    "stroke-paint-",
];

pub(super) fn node_id(id: &str) -> String {
    let mut bytes = id.bytes();
    let valid = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte));
    if valid && !RESERVED.iter().any(|prefix| id.starts_with(prefix)) {
        id.to_owned()
    } else {
        // Hex encodes every UTF-8 byte, including the empty ID. Unlike a
        // punctuation-replacement sanitizer, this has no collisions.
        format!("node-{}", svg_id(id))
    }
}
