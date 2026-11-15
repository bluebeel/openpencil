//! One forward scan of completed writer bytes. Descendant spans are indexed
//! during the first pass, not rescanned through every ancestor's field list.
use super::{Index, Node, Span, MAX_INDEX};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
};
#[derive(Clone, Copy)]
enum Role {
    Document,
    Page,
    Node,
}
struct Scan<'a> {
    input: BufReader<&'a mut File>,
    at: usize,
    nodes: HashMap<String, Option<Node>>,
    names: usize,
    meta: Option<Span>,
}
pub(super) fn build(file: &mut File) -> Option<Index> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut scan = Scan {
        input: BufReader::with_capacity(128 * 1024, file),
        at: 0,
        nodes: HashMap::new(),
        names: 0,
        meta: None,
    };
    scan.object(Role::Document, 0)?;
    scan.space()?;
    if scan.peek().is_some() {
        return None;
    }
    Some(Index {
        nodes: scan.nodes,
        meta: scan.meta?,
    })
}
impl Scan<'_> {
    fn peek(&mut self) -> Option<u8> {
        self.input.fill_buf().ok()?.first().copied()
    }
    fn advance(&mut self, n: usize) -> Option<()> {
        self.at = self.at.checked_add(n)?;
        self.input.consume(n);
        Some(())
    }
    fn byte(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.advance(1)?;
        Some(byte)
    }
    fn space(&mut self) -> Option<()> {
        loop {
            let bytes = self.input.fill_buf().ok()?;
            let n = bytes
                .iter()
                .take_while(|byte| byte.is_ascii_whitespace())
                .count();
            if n == 0 {
                return Some(());
            }
            self.advance(n)?;
        }
    }
    fn expect(&mut self, byte: u8) -> Option<()> {
        self.space()?;
        (self.byte()? == byte).then_some(())
    }
    // Reserved schema keys/ids are small; opaque strings (including base64)
    // are skipped in buffered runs without allocating an encoded copy.
    fn string(&mut self, capture: bool) -> Option<Vec<u8>> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        if capture {
            out.push(b'"');
        }
        loop {
            let bytes = self.input.fill_buf().ok()?;
            if bytes.is_empty() {
                return None;
            }
            let n = bytes
                .iter()
                .position(|byte| matches!(byte, b'"' | b'\\'))
                .unwrap_or(bytes.len());
            let exhausted = n == bytes.len();
            if capture {
                if out.len().checked_add(n)? > 16 * 1024 {
                    return None;
                }
                out.extend_from_slice(&bytes[..n]);
            }
            self.advance(n)?;
            if exhausted {
                continue;
            }
            match self.byte()? {
                b'"' => {
                    if capture {
                        out.push(b'"');
                    }
                    return Some(out);
                }
                b'\\' => {
                    let escaped = self.byte()?;
                    if capture {
                        out.extend_from_slice(&[b'\\', escaped]);
                        if out.len() > 16 * 1024 {
                            return None;
                        }
                    }
                }
                _ => return None,
            }
        }
    }
    fn skip(&mut self, depth: usize) -> Option<()> {
        self.space()?;
        match self.peek()? {
            b'"' => {
                self.string(false)?;
            }
            b'{' | b'[' => {
                let mut nested = 0usize;
                loop {
                    match self.peek()? {
                        b'"' => {
                            self.string(false)?;
                            continue;
                        }
                        b'{' | b'[' => {
                            nested = nested.checked_add(1)?;
                            if nested.checked_add(depth)? > 512 {
                                return None;
                            }
                        }
                        b'}' | b']' => {
                            nested = nested.checked_sub(1)?;
                        }
                        _ => (),
                    }
                    self.advance(1)?;
                    if nested == 0 {
                        break;
                    }
                }
            }
            _ => {
                let start = self.at;
                while self
                    .peek()
                    .is_some_and(|b| !b.is_ascii_whitespace() && !matches!(b, b',' | b'}' | b']'))
                {
                    self.advance(1)?;
                }
                if self.at == start {
                    return None;
                }
            }
        }
        Some(())
    }
    fn array(&mut self, role: Role, depth: usize) -> Option<Span> {
        self.space()?;
        let start = self.at;
        self.expect(b'[')?;
        self.space()?;
        if self.peek()? == b']' {
            self.advance(1)?;
            return Some(Span {
                start,
                end: self.at,
            });
        }
        loop {
            self.object(role, depth + 1)?;
            self.space()?;
            match self.byte()? {
                b',' => (),
                b']' => break,
                _ => return None,
            }
        }
        Some(Span {
            start,
            end: self.at,
        })
    }
    fn object(&mut self, role: Role, depth: usize) -> Option<()> {
        if depth > 512 {
            return None;
        }
        self.space()?;
        let start = self.at;
        self.expect(b'{')?;
        let mut id = None;
        let mut children = None;
        let mut count = 0usize;
        self.space()?;
        if self.peek()? == b'}' {
            self.advance(1)?;
        } else {
            loop {
                count += 1;
                if count > 4096 {
                    return None;
                }
                let key: String = serde_json::from_slice(&self.string(true)?).ok()?;
                self.expect(b':')?;
                self.space()?;
                let value_start = self.at;
                match (role, key.as_str(), self.peek()?) {
                    (Role::Document | Role::Page, "children", b'[') => {
                        self.array(Role::Node, depth + 1)?;
                    }
                    (Role::Document, "pages", b'[') => {
                        self.array(Role::Page, depth + 1)?;
                    }
                    (Role::Document, "editorMeta", _) => {
                        self.skip(depth + 1)?;
                        self.meta = Some(Span {
                            start: value_start,
                            end: self.at,
                        });
                    }
                    (Role::Node, "id", b'"') => {
                        id = Some(serde_json::from_slice::<String>(&self.string(true)?).ok()?);
                    }
                    (Role::Node, "children", b'[') => {
                        children = Some(self.array(Role::Node, depth + 1)?);
                    }
                    (Role::Node, "children", _) => return None,
                    _ => self.skip(depth + 1)?,
                }
                self.space()?;
                match self.byte()? {
                    b',' => (),
                    b'}' => break,
                    _ => return None,
                }
            }
        }
        if matches!(role, Role::Node) {
            let id = id?;
            self.names = self.names.checked_add(id.capacity().checked_add(32)?)?;
            let node = Node {
                object: Span {
                    start,
                    end: self.at,
                },
                children,
            };
            if self.nodes.contains_key(&id) {
                self.nodes.insert(id, None);
            } else {
                self.nodes.insert(id, Some(node));
            }
            let slots = self
                .nodes
                .capacity()
                .checked_mul(std::mem::size_of::<(String, Option<Node>)>() + 16)?;
            if self.names.checked_add(slots)? > MAX_INDEX {
                return None;
            }
        }
        Some(())
    }
}
