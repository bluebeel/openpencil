//! Index only canonical node objects and editorMeta in a writer-produced file.
//! Opaque objects are skipped as values, never interpreted as nodes.
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Span {
    pub start: usize,
    pub end: usize,
}
impl Span {
    pub fn overlaps(self, other: Self) -> bool {
        if self.start == self.end {
            return other.start < self.start && self.start < other.end;
        }
        if other.start == other.end {
            return self.start < other.start && other.start < self.end;
        }
        self.start < other.end && other.start < self.end
    }
}
#[derive(Clone, Copy)]
pub(super) struct Children {
    pub span: Span,
    pub len: usize,
    pub last: Option<usize>,
}
#[derive(Clone, Copy)]
pub(super) struct Node {
    pub object: Span,
    pub children: Option<Children>,
}
pub(super) struct Index {
    pub nodes: HashMap<String, Option<Node>>,
    pub root: Option<Children>,
    pub pages: HashMap<String, Option<Children>>,
    pub meta: Span,
    pub headers: [Option<Span>; 4],
}
#[path = "index_stream.rs"]
mod stream;
const MAX_INDEX: usize = 224 * 1024 * 1024;

impl Index {
    pub fn new(file: &mut std::fs::File) -> Option<Self> {
        stream::build(file)
    }
    pub fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.nodes.capacity() * (std::mem::size_of::<(String, Option<Node>)>() + 16)
            + self
                .nodes
                .keys()
                .map(|id| id.capacity() + 32)
                .sum::<usize>()
            + self.pages.capacity() * (std::mem::size_of::<(String, Option<Children>)>() + 16)
            + self
                .pages
                .keys()
                .map(|id| id.capacity() + 32)
                .sum::<usize>()
    }
}

pub(super) fn fields(bytes: &[u8], object: Span) -> Option<Vec<(&str, Span)>> {
    let mut at = whitespace(bytes, object.start);
    if bytes.get(at) != Some(&b'{') {
        return None;
    }
    at += 1;
    let mut out = Vec::new();
    loop {
        at = whitespace(bytes, at);
        if bytes.get(at) == Some(&b'}') {
            return (at + 1 == object.end).then_some(out);
        }
        let key = at;
        let end = string(bytes, at)?;
        // This index accepts only bytes emitted by the typed writer. Reserved
        // schema keys are literal ASCII; other keys are opaque and ignored.
        let name = std::str::from_utf8(&bytes[key + 1..end - 1]).ok()?;
        at = whitespace(bytes, end);
        if bytes.get(at) != Some(&b':') {
            return None;
        }
        at = whitespace(bytes, at + 1);
        let end = value(bytes, at)?;
        out.push((name, Span { start: at, end }));
        at = whitespace(bytes, end);
        match bytes.get(at)? {
            b',' => at += 1,
            b'}' => (),
            _ => return None,
        }
        if out.len() > 4096 {
            return None;
        }
    }
}
pub(super) fn items(bytes: &[u8], array: Span) -> Option<Vec<Span>> {
    let mut at = array.start;
    if bytes.get(at) != Some(&b'[') {
        return None;
    }
    at += 1;
    let mut out = Vec::new();
    loop {
        at = whitespace(bytes, at);
        if bytes.get(at) == Some(&b']') {
            return (at + 1 == array.end).then_some(out);
        }
        let end = value(bytes, at)?;
        out.push(Span { start: at, end });
        if out.len() > 1_000_000 {
            return None;
        }
        at = whitespace(bytes, end);
        match bytes.get(at)? {
            b',' => at += 1,
            b']' => (),
            _ => return None,
        }
    }
}
fn whitespace(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}
fn string(bytes: &[u8], mut at: usize) -> Option<usize> {
    if bytes.get(at) != Some(&b'"') {
        return None;
    }
    at += 1;
    while let Some(byte) = bytes.get(at) {
        match byte {
            b'\\' => at += 2,
            b'"' => return Some(at + 1),
            _ => at += 1,
        }
    }
    None
}
fn value(bytes: &[u8], start: usize) -> Option<usize> {
    match bytes.get(start)? {
        b'"' => string(bytes, start),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut at = start;
            while let Some(byte) = bytes.get(at) {
                match byte {
                    b'"' => {
                        at = string(bytes, at)?;
                        continue;
                    }
                    b'{' | b'[' => {
                        depth += 1;
                        if depth > 512 {
                            return None;
                        }
                    }
                    b'}' | b']' => {
                        depth = depth.checked_sub(1)?;
                        if depth == 0 {
                            return Some(at + 1);
                        }
                    }
                    _ => (),
                }
                at += 1;
            }
            None
        }
        _ => {
            let mut end = start;
            while bytes
                .get(end)
                .is_some_and(|b| !matches!(b, b',' | b'}' | b']') && !b.is_ascii_whitespace())
            {
                end += 1;
            }
            (end > start).then_some(end)
        }
    }
}
