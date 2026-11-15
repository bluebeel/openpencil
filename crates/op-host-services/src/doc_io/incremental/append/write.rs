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
    arrays: Peekable<btree_map::Range<'a, Location, ArrayPlan>>,
}
enum Frame<'a> {
    Data(Data<'a>),
    Links(slice::Iter<'a, Link>),
    Nodes {
        ids: slice::Iter<'a, String>,
        first: bool,
    },
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
            arrays: self
                .arrays
                .range(Location { source, at: start }..Location { source, at: end })
                .peekable(),
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
        index: &index::Index,
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
        let mut expanded = 0usize;
        while let Some(frame) = stack.last_mut() {
            match frame {
                Frame::Nodes { ids, first } => {
                    let Some(id) = ids.next() else {
                        writer
                            .write_all(b"]")
                            .map_err(|error| DocIoError::Io(error.to_string()))?;
                        stack.pop();
                        continue;
                    };
                    if !*first {
                        writer
                            .write_all(b",")
                            .map_err(|error| DocIoError::Io(error.to_string()))?;
                    }
                    *first = false;
                    let node = self
                        .nodes
                        .get(id)
                        .copied()
                        .or_else(|| {
                            index
                                .nodes
                                .get(id)
                                .copied()
                                .flatten()
                                .map(|node| PlacedNode {
                                    source: Source::Raw,
                                    node,
                                })
                        })
                        .ok_or_else(|| invalid(Source::Raw, 0, 0))?;
                    expanded += 1;
                    if expanded > 1_000_000 {
                        return Err(invalid(
                            node.source,
                            node.node.object.start,
                            node.node.object.end,
                        ));
                    }
                    stack.push(Frame::Data(self.frame(
                        node.source,
                        node.node.object.start,
                        node.node.object.end,
                        replacements,
                    )?));
                }
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
                    let array = data.arrays.peek().map(|(_, plan)| plan.span);
                    if array.is_some_and(|array| {
                        patch.is_none_or(|patch| array.start <= patch.start)
                            && point.is_none_or(|point| array.start <= point)
                    }) {
                        let (_, plan) = data
                            .arrays
                            .next()
                            .ok_or_else(|| invalid(data.source, data.at, data.end))?;
                        if plan.span.end > data.end {
                            return Err(invalid(data.source, plan.span.start, plan.span.end));
                        }
                        self.copy(data.source, data.at, plan.span.start, raw, writer)?;
                        data.at = plan.span.end;
                        // Covered old-node events are reapplied only when that
                        // node's actual after-piece is expanded at its new place.
                        while data
                            .patches
                            .peek()
                            .is_some_and(|(span, _)| span.start < plan.span.end)
                        {
                            let (span, _) = data
                                .patches
                                .next()
                                .ok_or_else(|| invalid(data.source, 0, 0))?;
                            if span.end > plan.span.end {
                                return Err(invalid(data.source, span.start, span.end));
                            }
                        }
                        while data
                            .points
                            .peek()
                            .is_some_and(|(point, _)| **point < plan.span.end)
                        {
                            data.points.next();
                        }
                        while data
                            .arrays
                            .peek()
                            .is_some_and(|(_, nested)| nested.span.start < plan.span.end)
                        {
                            let (_, nested) = data
                                .arrays
                                .next()
                                .ok_or_else(|| invalid(data.source, 0, 0))?;
                            if nested.span.end > plan.span.end {
                                return Err(invalid(
                                    data.source,
                                    nested.span.start,
                                    nested.span.end,
                                ));
                            }
                        }
                        writer
                            .write_all(b"[")
                            .map_err(|error| DocIoError::Io(error.to_string()))?;
                        stack.push(Frame::Nodes {
                            ids: plan.ids.iter(),
                            first: true,
                        });
                        continue;
                    }
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
