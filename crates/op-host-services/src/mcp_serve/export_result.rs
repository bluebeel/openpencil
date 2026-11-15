//! Encode directly into the final inline JSON text. Rendered payloads never
//! need an intermediate base64 String and a second serde Value-owned copy.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use op_util::json_escape::push_json_quoted;

pub(super) struct Files {
    text: String,
    first: bool,
}
impl Files {
    pub(super) fn new() -> Self {
        Self {
            text: String::from("{\"files\":["),
            first: true,
        }
    }
    fn begin(&mut self) {
        if !self.first {
            self.text.push(',');
        }
        self.first = false;
        self.text.push('{');
    }
    pub(super) fn node(
        &mut self,
        id: &str,
        format: &str,
        result: Result<Vec<u8>, crate::export::ExportError>,
    ) {
        match result {
            Ok(bytes) => {
                self.begin();
                reserve_payload(
                    &mut self.text,
                    &bytes,
                    id.len().saturating_add(format.len()),
                );
                append_bytes(&mut self.text, &bytes);
                self.text.push(',');
                field(&mut self.text, "format", format);
                self.text.push(',');
                field(&mut self.text, "nodeId", id);
                self.text.push('}');
            }
            Err(error) => self.error(id, format, &error.to_string()),
        }
    }
    pub(super) fn error(&mut self, id: &str, format: &str, error: &str) {
        self.begin();
        field(&mut self.text, "error", error);
        self.text.push(',');
        field(&mut self.text, "format", format);
        self.text.push(',');
        field(&mut self.text, "nodeId", id);
        self.text.push('}');
    }
    pub(super) fn combined(&mut self, ids: &[&str], format: &str, bytes: &[u8]) {
        self.begin();
        let metadata = ids.iter().fold(format.len(), |size, id| {
            size.saturating_add(id.len()).saturating_add(2)
        });
        reserve_payload(&mut self.text, bytes, metadata);
        append_bytes(&mut self.text, bytes);
        self.text.push(',');
        field(&mut self.text, "format", format);
        self.text.push_str(",\"nodeIds\":[");
        for (index, id) in ids.iter().enumerate() {
            if index != 0 {
                self.text.push(',');
            }
            push_json_quoted(&mut self.text, id);
        }
        self.text.push_str("]}");
    }
    pub(super) fn finish(mut self) -> String {
        self.text.push_str("]}");
        self.text
    }
}
fn reserve_payload(out: &mut String, bytes: &[u8], metadata_bytes: usize) {
    // Leave room for trailing escaped metadata before the encoder reserves.
    // Otherwise appending a tiny node ID can double a large payload's buffer.
    if let Some(additional) = bytes
        .len()
        .checked_add(2)
        .and_then(|length| (length / 3).checked_mul(4))
        .and_then(|length| length.checked_add(metadata_bytes.saturating_mul(6)))
        .and_then(|length| length.checked_add(128))
    {
        out.reserve(additional);
    }
}
fn append_bytes(out: &mut String, bytes: &[u8]) {
    out.push_str("\"bytes_base64\":\"");
    STANDARD.encode_string(bytes, out);
    out.push('"');
}
fn field(out: &mut String, key: &str, value: &str) {
    push_json_quoted(out, key);
    out.push(':');
    push_json_quoted(out, value);
}
pub(super) fn item(id: &str, kind: &str, format: &str, bytes: &[u8]) -> String {
    let mut out = String::from("{");
    reserve_payload(
        &mut out,
        bytes,
        id.len()
            .saturating_add(kind.len())
            .saturating_add(format.len()),
    );
    append_bytes(&mut out, bytes);
    for (key, value) in [("format", format), ("itemId", id), ("itemType", kind)] {
        out.push(',');
        field(&mut out, key, value);
    }
    out.push('}');
    out
}
/// The encoded value comes directly from STANDARD, whose alphabet needs no
/// JSON escaping. The screenshot contract intentionally carries it twice.
pub(super) fn screenshot_metadata(id: &str, encoded: &str) -> String {
    let mut out = String::with_capacity(
        encoded
            .len()
            .saturating_add(id.len().saturating_mul(6))
            .saturating_add(64),
    );
    out.push_str("{\"format\":\"png\",\"image_base64\":\"");
    out.push_str(encoded);
    out.push_str("\",");
    field(&mut out, "nodeId", id);
    out.push('}');
    out
}
