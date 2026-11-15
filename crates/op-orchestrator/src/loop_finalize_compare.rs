//! Compare canonical replay bytes without constructing two JSON value forests.
use super::RecordLoopFinalizeError;
use jian_ops_schema::PenDocument;
use std::io::{self, Write};

struct Compare<'a> {
    expected: &'a [u8],
    offset: usize,
    differs: bool,
}
impl Write for Compare<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let end = self.offset.checked_add(bytes.len());
        if end.and_then(|end| self.expected.get(self.offset..end)) != Some(bytes) {
            self.differs = true;
            return Err(io::Error::other("canonical replay differs"));
        }
        self.offset = end.expect("the matching byte range has an end");
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn matches(
    expected: &PenDocument,
    actual: &PenDocument,
) -> Result<bool, RecordLoopFinalizeError> {
    let expected = serde_json::to_vec(expected).map_err(|error| {
        RecordLoopFinalizeError::new(format!("serialize finalized document: {error}"))
    })?;
    let mut compare = Compare {
        expected: &expected,
        offset: 0,
        differs: false,
    };
    let result = serde_json::to_writer(&mut compare, actual);
    if compare.differs {
        return Ok(false);
    }
    result.map_err(|error| {
        RecordLoopFinalizeError::new(format!("serialize replayed document: {error}"))
    })?;
    Ok(compare.offset == expected.len())
}
