//! Bounded typed encodings from completed canonical header inverses. No tree
//! capture, raw JSON projection or command replay is used to reconstruct old values.
use serde::Serialize;
/// The actual append destination from a completed scoped inverse. Node IDs
/// identify existing Frame child arrays, not selection or predicted allocation.
pub enum CanonicalChildTarget<'a> {
    Root,
    Page { index: usize, id: &'a str },
    Node { id: &'a str },
}
/// Only the new suffix is borrowed. Existing child forests are never encoded.
pub struct CanonicalAppend<'a> {
    pub target: CanonicalChildTarget<'a>,
    pub previous_len: usize,
    pub previous_last: Option<&'a str>,
    pub nodes: &'a [jian_ops_schema::node::PenNode],
}

use std::io::{self, Write};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanonicalHeaderField {
    Variables,
    Themes,
    DesignMd,
    Conversion,
}
impl CanonicalHeaderField {
    pub fn index(self) -> usize {
        match self {
            Self::Variables => 0,
            Self::Themes => 1,
            Self::DesignMd => 2,
            Self::Conversion => 3,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Variables => "variables",
            Self::Themes => "themes",
            Self::DesignMd => "designMd",
            Self::Conversion => "conversion",
        }
    }
}
/// None is a genuinely omitted optional root field, distinct from an empty
/// map/object. Consumers must fall back if their byte index cannot add/remove it.
pub struct CanonicalHeaderChange {
    pub field: CanonicalHeaderField,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}
pub(crate) fn encode<T: Serialize + ?Sized>(value: &T, limit: usize) -> Option<Vec<u8>> {
    struct Limited {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for Limited {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let size = self
                .bytes
                .len()
                .checked_add(bytes.len())
                .filter(|n| *n <= self.limit)
                .ok_or(io::ErrorKind::OutOfMemory)?;
            if size > self.bytes.capacity() {
                let target = size
                    .max(self.bytes.capacity().saturating_mul(2))
                    .max(512)
                    .min(self.limit);
                self.bytes
                    .try_reserve_exact(target - self.bytes.len())
                    .map_err(|_| io::ErrorKind::OutOfMemory)?;
                if self.bytes.capacity() > self.limit {
                    return Err(io::ErrorKind::OutOfMemory.into());
                }
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut out = Limited {
        bytes: Vec::new(),
        limit,
    };
    jian_ops_schema::image_table::without_save_scope(|| serde_json::to_writer(&mut out, value))
        .ok()?;
    Some(out.bytes)
}
pub(crate) fn pair<T: Serialize>(
    field: CanonicalHeaderField,
    old: &Option<T>,
    current: &Option<T>,
    limit: usize,
) -> Option<CanonicalHeaderChange> {
    Some(CanonicalHeaderChange {
        field,
        before: old.as_ref().map(|v| encode(v, limit)).transpose_option()?,
        after: current
            .as_ref()
            .map(|v| encode(v, limit))
            .transpose_option()?,
    })
}
trait TransposeOption<T> {
    fn transpose_option(self) -> Option<Option<T>>;
}
impl<T> TransposeOption<T> for Option<Option<T>> {
    fn transpose_option(self) -> Option<Option<T>> {
        match self {
            None => Some(None),
            Some(v) => v.map(Some),
        }
    }
}
