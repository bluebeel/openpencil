//! Bounded diagnostic formatting stops before visiting the rest of a large
//! command/image/subtree payload, rather than formatting it and truncating later.
use std::fmt::{self, Debug, Write};
struct Output {
    value: String,
    limit: usize,
    truncated: bool,
}
impl Write for Output {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let available = self.limit.saturating_sub(self.value.len());
        if value.len() <= available {
            self.value.push_str(value);
            return Ok(());
        }
        let mut end = available;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.value.push_str(&value[..end]);
        self.truncated = true;
        Err(fmt::Error)
    }
}
/// Format at most `limit` bytes of Debug output plus an explicit `...` suffix
/// when truncated. Complete short values preserve their existing Debug bytes.
pub fn summary(value: &impl Debug, limit: usize) -> String {
    let mut out = Output {
        value: String::with_capacity(limit.min(4096)),
        limit,
        truncated: false,
    };
    let failed = fmt::write(&mut out, format_args!("{value:?}")).is_err();
    if out.truncated || failed {
        out.value.push_str("...");
    }
    out.value
}
