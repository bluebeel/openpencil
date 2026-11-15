//! Lenient JSON argument parsing for the `batch_design` DSL executor:
//! the strict-then-repair `parse_json_arg` pipeline and its bracket /
//! quote repair helpers.
//!
//! Split out of `batch_program.rs` to stay under the 800-line cap.

use serde_json::Value;
use std::borrow::Cow;

use super::batch_program::Result;
use super::batch_program_error::ProgramError;

// Each literal has one bounded, thread-safe compiled automaton. Keeping this
// at the call site avoids both dynamic cache keys and compiling three grammar
// expressions for every operation in a mixed program.
macro_rules! regex {
    ($pattern:literal) => {{
        static COMPILED: std::sync::LazyLock<::regex::Regex> = std::sync::LazyLock::new(|| {
            ::regex::Regex::new($pattern).expect("static DSL regex must compile")
        });
        &*COMPILED
    }};
}
pub(crate) use regex;

// The node-body -> `PenNode` step lives in `batch_program_node_parse.rs`.

/// TS `parseJsonArg` — strict JSON first, then the lenient agent-typo
/// pipeline: quote unquoted keys, single→double quote delimiters,
/// strip empty-key artifacts and trailing commas.
pub(crate) fn parse_json_arg(raw: &str) -> Result<Value> {
    let trimmed = raw.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(value);
    }
    // `(?<=\{|,)\s*(\w+)\s*:` has a lookbehind; the Rust regex crate
    // doesn't support those, so capture + reinsert the brace/comma.
    let mut normalized = regex!(r"([{,])\s*(\w+)\s*:")
        .replace_all(trimmed, "$1 \"$2\":")
        .into_owned();
    if normalized.as_bytes().contains(&b'\'') {
        normalized = replace_single_quote_delimiters(&normalized);
    }
    // Repair the common weak-model typo where a string value's closing quote
    // fuses with the trailing comma — `"k":"700,"next":...` meant
    // `"k":"700","next":...`. Anchored to a following `"<word>"` so it only
    // fires when the next token looks like a new key (a valid `"a","b"` has the
    // value's own close-quote before the comma and never matches).
    apply_regex_repair(
        &mut normalized,
        regex!(r#":\s*"([^"]*),"(\w+)""#),
        r#":"${1}","${2}""#,
    );
    // Repair a string value missing its OPENING quote — `"width":fill_container"`
    // meant `"width":"fill_container"` (weak model dropped the leading `"`).
    // Anchored: a letter-led bareword that ENDS with a `"` right after a colon
    // (a valid value starts WITH the quote, so `:"x"` never matches; bare
    // `true`/`false`/`null` aren't followed by a `"` so they never match either).
    apply_regex_repair(
        &mut normalized,
        regex!(r#":(\s*)([A-Za-z][\w-]*)""#),
        r#":${1}"${2}""#,
    );
    // Repair a FULLY-unquoted string value — `"width":fill_container_str,` meant
    // `"width":"fill_container_str"` (a weak model emitted a bare identifier, e.g.
    // a leaked JS variable name). A letter-led bareword between a colon and a
    // `,`/`}`/`]`. Numbers are digit-led (never match); a real quoted value
    // starts with `"` (never matches); `true`/`false`/`null` get re-unquoted next.
    apply_regex_repair(
        &mut normalized,
        regex!(r#":(\s*)([A-Za-z][\w-]*)(\s*[,}\]])"#),
        r#":${1}"${2}"${3}"#,
    );
    apply_regex_repair(
        &mut normalized,
        regex!(r#":(\s*)"(true|false|null)"(\s*[,}\]])"#),
        r#":${1}${2}${3}"#,
    );
    apply_regex_repair(&mut normalized, regex!(r#",\s*""\s*:\s*[^,}\]]+"#), "");
    apply_regex_repair(&mut normalized, regex!(r",(\s*[}\]])"), "$1");
    // Last resort: a weak model commonly drops the trailing closing brace(s) of a
    // node — especially one with a nested object like `"stroke":{"thickness":{...}`
    // — leaving `{...}` short a `}`. When that node is the program's ROOT binding,
    // the parse failure cascades (every child's `I(rootBinding, ...)` then can't
    // find its parent). Auto-closing the unbalanced brackets recovers it.
    serde_json::from_str(&normalized)
        .or_else(|_| serde_json::from_str(&close_unbalanced_brackets(&normalized)))
        .map_err(|e| {
            let mut chars = raw.chars();
            let snippet: String = chars.by_ref().take(300).collect();
            let ellipsis = if chars.next().is_some() { "..." } else { "" };
            ProgramError::Json(format!("Failed to parse JSON ({e}): {snippet}{ellipsis}"))
        })
}

pub(crate) fn parse_string_arg(raw: &str, label: &str) -> Result<String> {
    match parse_json_arg(raw)? {
        Value::String(text) => Ok(text),
        _ => Err(ProgramError::Syntax(format!(
            "{label} must be a JSON string"
        ))),
    }
}

/// Append the closing brackets for any `{`/`[` left open at end-of-string
/// (respecting string literals + escapes), in correct nesting order. A no-op on
/// already-balanced input. This is a best-effort repair for weak models that
/// drop trailing `}` / `]`; it cannot recover a value truncated mid-token, but
/// it rescues the dominant "forgot the closers" shape.
fn close_unbalanced_brackets(s: &str) -> Cow<'_, str> {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string: Option<char> = None;
    let mut escape = false;
    for ch in s.chars() {
        if escape {
            escape = false;
            continue;
        }
        if let Some(quote) = in_string {
            if ch == '\\' {
                escape = true;
            } else if ch == quote {
                in_string = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => in_string = Some(ch),
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                stack.pop();
            }
            _ => {}
        }
    }
    if stack.is_empty() {
        return Cow::Borrowed(s);
    }
    let mut out = s.to_string();
    while let Some(closer) = stack.pop() {
        out.push(closer);
    }
    Cow::Owned(out)
}

fn apply_regex_repair(text: &mut String, pattern: &regex::Regex, replacement: &str) {
    if let Cow::Owned(repaired) = pattern.replace_all(text.as_str(), replacement) {
        *text = repaired;
    }
}

/// TS `replaceSingleQuoteDelimiters` — swap single-quote string
/// delimiters for double quotes, leaving apostrophes inside
/// double-quoted strings alone.
fn replace_single_quote_delimiters(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    let mut in_double = false;
    let mut in_single = false;
    while let Some(ch) = chars.next() {
        if ch == '\\' && (in_double || in_single) {
            out.push(ch);
            if let Some(next) = chars.next() {
                out.push(next);
            }
            continue;
        }
        if in_double {
            if ch == '"' {
                in_double = false;
            }
            out.push(ch);
        } else if in_single {
            if ch == '\'' {
                in_single = false;
                out.push('"');
            } else {
                out.push(ch);
            }
        } else if ch == '"' {
            in_double = true;
            out.push(ch);
        } else if ch == '\'' {
            in_single = true;
            out.push('"');
        } else {
            out.push(ch);
        }
    }
    out
}
