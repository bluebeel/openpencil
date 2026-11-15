//! Attribute/text escaping directly into the final SVG formatter. This emits
//! the same five entities as escape_xml without copying an entire path/URI.
use std::fmt;

pub(super) struct Escaped<'a>(pub(super) &'a str);
impl fmt::Display for Escaped<'_> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut start = 0;
        for (at, byte) in self.0.bytes().enumerate() {
            let entity = match byte {
                b'&' => "&amp;",
                b'<' => "&lt;",
                b'>' => "&gt;",
                b'"' => "&quot;",
                b'\'' => "&apos;",
                _ => continue,
            };
            // Entity bytes are ASCII and therefore UTF8 character boundaries.
            out.write_str(&self.0[start..at])?;
            out.write_str(entity)?;
            start = at + 1;
        }
        out.write_str(&self.0[start..])
    }
}
