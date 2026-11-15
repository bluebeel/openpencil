//! Stream the immutable source/segment DAG with depth-sized cursors. Neither
//! unchanged forests nor previously encoded suffixes are concatenated/copied.
use super::*;
use crate::doc_io::DocIoError;
use std::{collections::btree_map, fs::File, io::Write, iter::Peekable, slice};
struct Data<'a> {
    source: Source,
    at: usize,
    end: usize,
    patches: Peekable<btree_map::Range<'a, Span, Vec<u8>>>,
    points: Peekable<btree_map::Range<'a, usize, Vec<Link>>>,
}
enum Frame<'a> {
    Data(Data<'a>),
    Links(slice::Iter<'a, Link>),
}
fn invalid(source: Source, start: usize, end: usize) -> DocIoError {
    DocIoError::IncrementalSpan {
        fragment: match source {
            Source::Raw => None,
            Source::Segment(id) => Some(id),
        },
        start,
        end,
    }
}
impl Overlay {
    fn frame<'a>(
        &'a self,
        source: Source,
        start: usize,
        end: usize,
        raw: &'a BTreeMap<Span, Vec<u8>>,
    ) -> Result<Data<'a>, DocIoError> {
        if start > end {
            return Err(invalid(source, start, end));
        }
        let (patches, points) = match source {
            Source::Raw => (raw, &self.raw_points),
            Source::Segment(id) => {
                let segment = self
                    .segments
                    .get(id)
                    .ok_or_else(|| invalid(source, start, end))?;
                if end > segment.bytes.len() {
                    return Err(invalid(source, start, end));
                }
                (&segment.patches, &segment.points)
            }
        };
        Ok(Data {
            source,
            at: start,
            end,
            patches: patches
                .range(Span { start, end: 0 }..Span { start: end, end: 0 })
                .peekable(),
            points: points.range(start..end).peekable(),
        })
    }
    fn copy(
        &self,
        source: Source,
        start: usize,
        end: usize,
        raw: &mut File,
        writer: &mut impl Write,
    ) -> Result<(), DocIoError> {
        if start > end {
            return Err(invalid(source, start, end));
        }
        match source {
            Source::Raw => super::super::copy_range(raw, start, end, writer),
            Source::Segment(id) => writer
                .write_all(
                    self.segments
                        .get(id)
                        .and_then(|segment| segment.bytes.get(start..end))
                        .ok_or_else(|| invalid(source, start, end))?,
                )
                .map_err(|error| DocIoError::Io(error.to_string())),
        }
    }
    pub(in crate::doc_io::incremental) fn write(
        &self,
        raw: &mut File,
        replacements: &BTreeMap<Span, Vec<u8>>,
        writer: &mut impl Write,
    ) -> Result<(), DocIoError> {
        let end = raw
            .metadata()
            .map_err(|error| DocIoError::Io(error.to_string()))?
            .len();
        let end = usize::try_from(end).map_err(|_| invalid(Source::Raw, 0, usize::MAX))?;
        let mut stack = vec![Frame::Data(self.frame(
            Source::Raw,
            0,
            end,
            replacements,
        )?)];
        while let Some(frame) = stack.last_mut() {
            match frame {
                Frame::Links(links) => {
                    let Some(link) = links.next().copied() else {
                        stack.pop();
                        continue;
                    };
                    let source = Source::Segment(link.segment);
                    let segment = self
                        .segments
                        .get(link.segment)
                        .ok_or_else(|| invalid(source, 0, 0))?;
                    let end = segment
                        .bytes
                        .len()
                        .checked_sub(1)
                        .ok_or_else(|| invalid(source, 0, 0))?;
                    if link.comma {
                        writer
                            .write_all(b",")
                            .map_err(|error| DocIoError::Io(error.to_string()))?;
                    }
                    stack.push(Frame::Data(self.frame(source, 1, end, replacements)?));
                }
                Frame::Data(data) => {
                    let patch = data.patches.peek().map(|(span, _)| **span);
                    let point = data.points.peek().map(|(point, _)| **point);
                    // A boundary insertion precedes a replacement beginning at
                    // the same byte; strict interior overlaps were refused.
                    if point.is_some_and(|point| patch.is_none_or(|patch| point <= patch.start)) {
                        let (point, links) = data
                            .points
                            .next()
                            .ok_or_else(|| invalid(data.source, data.at, data.end))?;
                        self.copy(data.source, data.at, *point, raw, writer)?;
                        data.at = *point;
                        stack.push(Frame::Links(links.iter()));
                    } else if let Some((span, bytes)) = data.patches.next() {
                        if span.end > data.end {
                            return Err(invalid(data.source, span.start, span.end));
                        }
                        self.copy(data.source, data.at, span.start, raw, writer)?;
                        data.at = span.end;
                        writer
                            .write_all(bytes)
                            .map_err(|error| DocIoError::Io(error.to_string()))?;
                    } else {
                        self.copy(data.source, data.at, data.end, raw, writer)?;
                        stack.pop();
                    }
                }
            }
        }
        Ok(())
    }
}
