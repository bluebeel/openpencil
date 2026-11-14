//! Canonical JSON string-literal escaping.
//!
//! Single-sources the escaping previously copy-pasted in op-mcp
//! (`json_serializer`), op-cli (`mcp_http_cli`), and op-host-services
//! (`mcp_serve::doc_sync` — whose copy lossily replaced control characters
//! with spaces; that one was a bug). Control characters below U+0020 get
//! the mandatory `\uXXXX` escapes so the output is always valid JSON.

/// Escape `s` for inclusion inside a JSON string literal (no surrounding
/// quotes added).
pub fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    push_escaped(&mut out, s);
    out
}

/// Escape `s` as a complete JSON string literal, surrounding quotes
/// included.
pub fn escape_json_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    push_json_quoted(&mut out, s);
    out
}

/// Append a complete JSON string literal to an existing output buffer.
pub fn push_json_quoted(out: &mut String, s: &str) {
    out.push('"');
    push_escaped(out, s);
    out.push('"');
}

/// Append canonical JSON string contents without surrounding quotes.
pub fn push_escaped(out: &mut String, s: &str) {
    // Every special byte is ASCII and therefore a UTF-8 boundary. Copy long
    // ordinary spans (including all Unicode) at once, avoiding per-character
    // UTF-8 decoding and pushes for base64 images and large JSON replies.
    let mut start = 0;
    for (at, byte) in s.bytes().enumerate() {
        let escape = match byte {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            b'\n' => "\\n",
            b'\r' => "\\r",
            b'\t' => "\\t",
            byte if byte < 0x20 => {
                out.push_str(&s[start..at]);
                const HEX: &[u8; 16] = b"0123456789abcdef";
                out.push_str("\\u00");
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 15) as usize] as char);
                start = at + 1;
                continue;
            }
            _ => continue,
        };
        out.push_str(&s[start..at]);
        out.push_str(escape);
        start = at + 1;
    }
    out.push_str(&s[start..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_specials_and_controls() {
        assert_eq!(escape_json("a\"b\\c"), "a\\\"b\\\\c");
        assert_eq!(escape_json("line1\nline2\r\t"), "line1\\nline2\\r\\t");
        assert_eq!(escape_json("nul\u{0}bell\u{7}"), "nul\\u0000bell\\u0007");
        assert_eq!(escape_json("中文 ok"), "中文 ok");
    }

    #[test]
    fn quoted_variant_wraps() {
        assert_eq!(escape_json_quoted("hi"), "\"hi\"");
        assert_eq!(escape_json_quoted("a\"b"), "\"a\\\"b\"");
    }
}
