//! Pure-Rust font-family extraction for the web import path.
//!
//! The vendored CanvasKit build (`assets/canvaskit/canvaskit.js`) is a slimmed
//! build with NO family-name introspection — `Typeface` has no
//! `getFamilyName()` and `FontMgr` exposes no `countFamilies`/`getFamilyName`.
//! So a freshly imported font's family (needed to key the CanvasKit typeface
//! map, the picker entry, and the IndexedDB record) is parsed here from the
//! font's `name` table with `ttf-parser` — wasm-safe and unit-testable without
//! a browser, unlike the JS/CanvasKit round-trip.

/// Extract a font's family display name from its raw `.ttf` / `.otf` bytes.
/// Prefers the Typographic Family (name id 16) over the legacy Family (id 1),
/// and a decodable Unicode/Windows record. Returns `None` when the bytes are
/// not a parseable font or carry no usable family name.
pub(crate) fn parse_family(bytes: &[u8]) -> Option<String> {
    // Face index 0 — a plain .ttf/.otf has one face; a .ttc collection's first.
    let face = ttf_parser::Face::parse(bytes, 0).ok()?;

    const FAMILY: u16 = 1;
    const TYPOGRAPHIC_FAMILY: u16 = 16;

    let mut family: Option<String> = None;
    let mut typographic: Option<String> = None;
    for name in face.names() {
        if name.name_id != FAMILY && name.name_id != TYPOGRAPHIC_FAMILY {
            continue;
        }
        // `to_string` decodes Unicode/Windows-platform records; Mac-Roman and
        // other legacy encodings yield `None` and are skipped — every font
        // that targets browsers/OSes ships a Windows Unicode family record.
        let Some(value) = name.to_string() else {
            continue;
        };
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if name.name_id == TYPOGRAPHIC_FAMILY {
            typographic.get_or_insert_with(|| trimmed.to_string());
        } else {
            family.get_or_insert_with(|| trimmed.to_string());
        }
    }
    typographic.or(family)
}

/// Actual binary descriptors for the browser shaping alias. Unknown/invalid
/// axes refuse only this optional alias; CanvasKit registration remains intact.
pub(crate) struct BrowserStyle {
    pub style: &'static str,
    pub weight: String,
    pub variations: String,
}
pub(crate) fn browser_style(bytes: &[u8]) -> Option<BrowserStyle> {
    use std::fmt::Write;
    let face = ttf_parser::Face::parse(bytes, 0).ok()?;
    let mut low = f32::from(face.weight().to_number());
    let mut high = low;
    let mut variations = String::new();
    for axis in face.variation_axes() {
        if axis.tag == ttf_parser::Tag::from_bytes(b"wght") {
            low = axis.min_value.max(1.0);
            high = axis.max_value.min(1000.0);
            continue;
        }
        if !axis.def_value.is_finite() {
            return None;
        }
        if !variations.is_empty() {
            variations.push_str(", ");
        }
        variations.push_str("'");
        for byte in axis.tag.to_bytes() {
            if !(0x20..=0x7e).contains(&byte) {
                return None;
            }
            if matches!(byte, b'\\' | b'\'') {
                variations.push('\\');
            }
            variations.push(char::from(byte));
        }
        write!(variations, "' {}", axis.def_value).ok()?;
    }
    if !(low.is_finite() && high.is_finite() && low >= 1.0 && high <= 1000.0 && low <= high) {
        return None;
    }
    let weight = if low == high {
        low.to_string()
    } else {
        format!("{low} {high}")
    };
    if variations.is_empty() {
        variations.push_str("normal");
    }
    Some(BrowserStyle {
        style: if face.is_italic() {
            "italic"
        } else if face.is_oblique() {
            "oblique"
        } else {
            "normal"
        },
        weight,
        variations,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_family;

    // A real bundled OFL font (shared with op-host-desktop). include_bytes from
    // a sibling crate's assets is a compile-time file read, not a dep edge.
    const INSTRUMENT_SERIF: &[u8] =
        include_bytes!("../../op-host-desktop/assets/fonts/InstrumentSerif-Regular.ttf");
    const OUTFIT_VF: &[u8] = include_bytes!("../../op-host-desktop/assets/fonts/Outfit-VF.ttf");

    #[test]
    fn extracts_family_from_real_fonts() {
        assert_eq!(
            parse_family(INSTRUMENT_SERIF).as_deref(),
            Some("Instrument Serif")
        );
        assert_eq!(parse_family(OUTFIT_VF).as_deref(), Some("Outfit"));
    }

    #[test]
    fn rejects_non_font_bytes() {
        assert_eq!(parse_family(b"this is not a font file"), None);
        assert_eq!(parse_family(&[]), None);
    }
}
