//! Hand-rolled wire parser for the `batch_design` `nodes_json` payload:
//! the flat descriptor-array grammar plus its string/number/raw-JSON
//! scanners.
//!
//! Split out of `batch_design.rs` to stay under the 800-line cap.

use super::batch_design_wire_error::WireParseError;
use super::write_tools::{validate_hex, ALLOWED_KINDS};
use super::BatchInsertItem;
use std::borrow::Cow;

/// Hand-rolled parser for the `nodes_json` payload. Shell-core
/// stays serde-free so the wasm32 bundle doesn't grow. Returns a
/// Vec<BatchInsertItem> on success, a typed [`WireParseError`] on
/// any structural problem.
///
/// Grammar (whitespace ignored):
///   array      = '[' (item (',' item)* )? ']'
///   item       = '{' pair (',' pair)* '}'
///   pair       = string ':' value
///   string     = '"' chars '"'
///   value      = string | number
///
/// Strings retain ordinary UTF-8 spans and decode JSON escapes, including
/// Unicode surrogate pairs, directly from their original quoted token.
pub(crate) fn parse_batch_items(input: &str) -> Result<Vec<BatchInsertItem>, WireParseError> {
    // The envelope parser already decoded its JSON string. Only historical
    // callers that escaped descriptor KEY quotes need the extra outer layer.
    // Ordinary inner JSON escapes (names, paths, Unicode) must stay intact.
    let unescaped = if uses_escaped_descriptor_keys(input.as_bytes()) {
        Cow::Owned(unescape_wire_string(input)?)
    } else {
        Cow::Borrowed(input)
    };
    let bytes = unescaped.as_bytes();
    let mut i = 0usize;
    skip_ws(bytes, &mut i);
    if i >= bytes.len() || bytes[i] != b'[' {
        return Err(WireParseError::ArrayMustStartWithBracket);
    }
    i += 1;
    skip_ws(bytes, &mut i);
    let mut out = Vec::new();
    if i < bytes.len() && bytes[i] == b']' {
        i += 1;
        skip_ws(bytes, &mut i);
        if i != bytes.len() {
            return Err(WireParseError::TrailingGarbage);
        }
        return Ok(out); // empty array — caller surfaces InvalidArgument
    }
    loop {
        skip_ws(bytes, &mut i);
        let item = parse_item(bytes, &mut i)?;
        out.push(item);
        skip_ws(bytes, &mut i);
        if i >= bytes.len() {
            return Err(WireParseError::UnterminatedArray);
        }
        match bytes[i] {
            b',' => {
                i += 1;
            }
            b']' => {
                i += 1;
                skip_ws(bytes, &mut i);
                if i != bytes.len() {
                    return Err(WireParseError::TrailingGarbage);
                }
                return Ok(out);
            }
            other => {
                return Err(WireParseError::ExpectedCommaOrBracket(other as char));
            }
        }
    }
}

fn parse_item(bytes: &[u8], i: &mut usize) -> Result<BatchInsertItem, WireParseError> {
    if *i >= bytes.len() || bytes[*i] != b'{' {
        return Err(WireParseError::ExpectedDescriptorBrace);
    }
    *i += 1;
    let mut kind: Option<String> = None;
    let mut name: Option<String> = None;
    let mut x: Option<i32> = None;
    let mut y: Option<i32> = None;
    let mut width: Option<i32> = None;
    let mut height: Option<i32> = None;
    let mut fill_hex: Option<String> = None;
    let mut fill: Option<Vec<jian_ops_schema::style::PenFill>> = None;
    loop {
        skip_ws(bytes, i);
        if *i >= bytes.len() {
            return Err(WireParseError::UnterminatedDescriptor);
        }
        if bytes[*i] == b'}' {
            *i += 1;
            break;
        }
        let key = parse_string(bytes, i)?;
        skip_ws(bytes, i);
        if *i >= bytes.len() || bytes[*i] != b':' {
            return Err(WireParseError::ExpectedColonAfterKey(key.into_owned()));
        }
        *i += 1;
        skip_ws(bytes, i);
        match key.as_ref() {
            "kind" => kind = Some(parse_string(bytes, i)?.into_owned()),
            "name" => name = Some(parse_string(bytes, i)?.into_owned()),
            "fill_hex" => fill_hex = Some(parse_string(bytes, i)?.into_owned()),
            // Generic `fill` passthrough: a full canonical PenFill stack
            // (array of fill objects, or a single fill object) so a batch
            // item can carry gradient / mesh / image fills, not just a
            // solid `fill_hex`. Captured as a balanced raw-JSON slice and
            // deserialized straight into the canonical type.
            "fill" => {
                let raw = capture_raw_json_value(bytes, i)?;
                fill = Some(parse_fill_stack(&raw)?);
            }
            "x" => x = Some(parse_int(bytes, i)?),
            "y" => y = Some(parse_int(bytes, i)?),
            "width" => width = Some(parse_int(bytes, i)?),
            "height" => height = Some(parse_int(bytes, i)?),
            other => return Err(WireParseError::UnknownKey(other.to_string())),
        }
        skip_ws(bytes, i);
        if *i < bytes.len() && bytes[*i] == b',' {
            *i += 1;
        }
    }
    let kind = kind.ok_or(WireParseError::MissingField("kind"))?;
    if !ALLOWED_KINDS.iter().any(|k| *k == kind) {
        return Err(WireParseError::UnsupportedKind(kind));
    }
    let name = name.ok_or(WireParseError::MissingField("name"))?;
    let x = x.ok_or(WireParseError::MissingField("x"))?;
    let y = y.ok_or(WireParseError::MissingField("y"))?;
    let width = width.ok_or(WireParseError::MissingField("width"))?;
    let height = height.ok_or(WireParseError::MissingField("height"))?;
    if width < 0 || height < 0 {
        return Err(WireParseError::NegativeSize);
    }
    if let Some(ref hex) = fill_hex {
        if !validate_hex(hex) {
            return Err(WireParseError::InvalidFillHex(hex.clone()));
        }
    }
    Ok(BatchInsertItem {
        kind,
        name,
        x,
        y,
        width,
        height,
        fill_hex,
        fill,
    })
}

/// Deserialize a raw JSON `fill` slice into a canonical fill stack.
/// Accepts either an array of fill objects (`[{...}, ...]`) or a single
/// fill object (`{...}`, wrapped into a 1-entry stack), mirroring the
/// `normalize_fill` shape-tolerance on the JSON nodes path.
fn parse_fill_stack(raw: &str) -> Result<Vec<jian_ops_schema::style::PenFill>, WireParseError> {
    use jian_ops_schema::style::PenFill;
    let trimmed = raw.trim_start();
    if trimmed.starts_with('[') {
        serde_json::from_str::<Vec<PenFill>>(raw)
            .map_err(|e| WireParseError::InvalidFillArray(e.to_string()))
    } else {
        serde_json::from_str::<PenFill>(raw)
            .map(|f| vec![f])
            .map_err(|e| WireParseError::InvalidFillObject(e.to_string()))
    }
}

/// Scan one balanced JSON value (object / array / string / number /
/// `true` / `false` / `null`) starting at `*i`, advance `*i` past it,
/// and return the raw slice. Respects nesting + string escapes so a
/// `}`/`]` inside a string doesn't prematurely close the value.
fn capture_raw_json_value<'a>(bytes: &'a [u8], i: &mut usize) -> Result<&'a str, WireParseError> {
    skip_ws(bytes, i);
    if *i >= bytes.len() {
        return Err(WireParseError::ExpectedJsonValue);
    }
    let start = *i;
    match bytes[*i] {
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut in_str = false;
            let mut escaped = false;
            while *i < bytes.len() {
                let c = bytes[*i];
                if in_str {
                    if escaped {
                        escaped = false;
                    } else if c == b'\\' {
                        escaped = true;
                    } else if c == b'"' {
                        in_str = false;
                    }
                } else {
                    match c {
                        b'"' => in_str = true,
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                *i += 1;
                                return slice_utf8(bytes, start, *i);
                            }
                        }
                        _ => {}
                    }
                }
                *i += 1;
            }
            Err(WireParseError::UnterminatedJsonValue)
        }
        b'"' => {
            // Reuse the string parser to advance past escapes correctly,
            // then return the original quoted slice.
            let _ = parse_string(bytes, i)?;
            slice_utf8(bytes, start, *i)
        }
        _ => {
            // Number / literal — run to the next delimiter.
            while *i < bytes.len()
                && !matches!(bytes[*i], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
            {
                *i += 1;
            }
            slice_utf8(bytes, start, *i)
        }
    }
}

fn slice_utf8(bytes: &[u8], start: usize, end: usize) -> Result<&str, WireParseError> {
    std::str::from_utf8(&bytes[start..end]).map_err(|_| WireParseError::InvalidUtf8("JSON value"))
}

fn uses_escaped_descriptor_keys(bytes: &[u8]) -> bool {
    let mut i = 0;
    skip_ws(bytes, &mut i);
    if bytes.get(i) != Some(&b'[') {
        return false;
    }
    i += 1;
    skip_ws(bytes, &mut i);
    if bytes.get(i) != Some(&b'{') {
        return false;
    }
    i += 1;
    skip_ws(bytes, &mut i);
    bytes.get(i..i + 2) == Some(b"\\\"")
}

/// Reverse the JSON-string escaping the wire parser left intact.
/// Handles `\"` / `\\` / `\n` / `\t` / `\r` / `\/`. Anything else
/// passes through verbatim (no `\u` decode today).
fn unescape_wire_string(input: &str) -> Result<String, WireParseError> {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\\' {
            let next = *bytes.get(i + 1).ok_or(WireParseError::UnterminatedEscape)?;
            match next {
                b'"' => out.push('"'),
                b'\\' => out.push('\\'),
                b'n' => out.push('\n'),
                b't' => out.push('\t'),
                b'r' => out.push('\r'),
                b'/' => out.push('/'),
                _ => {
                    // Unknown escape — pass through verbatim so
                    // typos surface as parser errors downstream.
                    out.push('\\');
                    out.push(next as char);
                }
            }
            i += 2;
        } else {
            let start = i;
            while i < bytes.len() && bytes[i] != b'\\' {
                i += 1;
            }
            let slice = std::str::from_utf8(&bytes[start..i])
                .map_err(|_| WireParseError::InvalidUtf8("nodes_json"))?;
            out.push_str(slice);
        }
    }
    Ok(out)
}

fn skip_ws(bytes: &[u8], i: &mut usize) {
    while *i < bytes.len() && bytes[*i].is_ascii_whitespace() {
        *i += 1;
    }
}

fn parse_string<'a>(bytes: &'a [u8], i: &mut usize) -> Result<Cow<'a, str>, WireParseError> {
    if bytes.get(*i) != Some(&b'"') {
        return Err(WireParseError::ExpectedString);
    }
    let start = *i;
    *i += 1;
    let mut escaped = false;
    while let Some(&byte) = bytes.get(*i) {
        if byte == b'"' {
            *i += 1;
            if escaped {
                let token = std::str::from_utf8(&bytes[start..*i])
                    .map_err(|_| WireParseError::InvalidUtf8("string"))?;
                return serde_json::from_str::<String>(token)
                    .map(Cow::Owned)
                    .map_err(|error| WireParseError::InvalidJsonString(error.to_string()));
            }
            return std::str::from_utf8(&bytes[start + 1..*i - 1])
                .map(Cow::Borrowed)
                .map_err(|_| WireParseError::InvalidUtf8("string"));
        }
        if byte == b'\\' {
            escaped = true;
            let escape = *bytes
                .get(*i + 1)
                .ok_or(WireParseError::UnterminatedEscape)?;
            if !matches!(
                escape,
                b'"' | b'\\' | b'n' | b't' | b'r' | b'/' | b'b' | b'f' | b'u'
            ) {
                return Err(WireParseError::UnsupportedEscape(escape as char));
            }
            *i += 2;
        } else if byte < 0x20 {
            return Err(WireParseError::InvalidJsonString(
                "unescaped control character".into(),
            ));
        } else {
            *i += 1;
        }
    }
    Err(WireParseError::UnterminatedString)
}

fn parse_int(bytes: &[u8], i: &mut usize) -> Result<i32, WireParseError> {
    let start = *i;
    if *i < bytes.len() && bytes[*i] == b'-' {
        *i += 1;
    }
    while *i < bytes.len() && bytes[*i].is_ascii_digit() {
        *i += 1;
    }
    if start == *i {
        return Err(WireParseError::ExpectedInteger);
    }
    let raw = std::str::from_utf8(&bytes[start..*i])
        .map_err(|_| WireParseError::InvalidUtf8("integer"))?;
    raw.parse::<i32>()
        .map_err(|_| WireParseError::NotAnI32(raw.to_string()))
}
