//! One forward pass over only a newly encoded suffix. Opaque extension values
//! stay opaque; no serde Value/tree or original file forest is materialized.
use super::{string, value, whitespace, Children, Node, Span};
const MAX_NODES: usize = 4096;
const MAX_DEPTH: usize = 128;
pub(in crate::doc_io::incremental) fn appended_nodes(bytes: &[u8]) -> Option<Vec<(String, Node)>> {
    let mut scan = Scan {
        bytes,
        at: 0,
        nodes: Vec::new(),
    };
    scan.array(0)?;
    (whitespace(bytes, scan.at) == bytes.len()).then_some(scan.nodes)
}
struct Scan<'a> {
    bytes: &'a [u8],
    at: usize,
    nodes: Vec<(String, Node)>,
}
impl Scan<'_> {
    fn expect(&mut self, byte: u8) -> Option<()> {
        self.at = whitespace(self.bytes, self.at);
        if self.bytes.get(self.at) != Some(&byte) {
            return None;
        }
        self.at += 1;
        Some(())
    }
    fn array(&mut self, depth: usize) -> Option<Children> {
        self.at = whitespace(self.bytes, self.at);
        let start = self.at;
        self.expect(b'[')?;
        self.at = whitespace(self.bytes, self.at);
        let mut len = 0;
        let mut last = None;
        if self.bytes.get(self.at) != Some(&b']') {
            loop {
                self.at = whitespace(self.bytes, self.at);
                last = Some(self.at);
                self.object(depth)?;
                len += 1;
                self.at = whitespace(self.bytes, self.at);
                match self.bytes.get(self.at)? {
                    b',' => self.at += 1,
                    b']' => break,
                    _ => return None,
                }
            }
        }
        self.expect(b']')?;
        Some(Children {
            span: Span {
                start,
                end: self.at,
            },
            len,
            last,
        })
    }
    fn object(&mut self, depth: usize) -> Option<()> {
        if depth > MAX_DEPTH || self.nodes.len() >= MAX_NODES {
            return None;
        }
        self.at = whitespace(self.bytes, self.at);
        let start = self.at;
        self.expect(b'{')?;
        let mut id = None;
        let mut children = None;
        let mut count = 0usize;
        loop {
            self.at = whitespace(self.bytes, self.at);
            if self.bytes.get(self.at) == Some(&b'}') {
                self.at += 1;
                break;
            }
            count += 1;
            if count > 4096 {
                return None;
            }
            let end = string(self.bytes, self.at)?;
            let key = std::str::from_utf8(self.bytes.get(self.at + 1..end - 1)?).ok()?;
            self.at = end;
            self.expect(b':')?;
            self.at = whitespace(self.bytes, self.at);
            match key {
                "children" => {
                    if children.is_some() {
                        return None;
                    }
                    children = Some(self.array(depth + 1)?);
                }
                "id" => {
                    if id.is_some() {
                        return None;
                    }
                    let end = string(self.bytes, self.at)?;
                    id = Some(serde_json::from_slice::<String>(&self.bytes[self.at..end]).ok()?);
                    self.at = end;
                }
                _ => self.at = value(self.bytes, self.at)?,
            }
            self.at = whitespace(self.bytes, self.at);
            match self.bytes.get(self.at)? {
                b',' => self.at += 1,
                b'}' => (),
                _ => return None,
            }
        }
        if self.nodes.len() >= MAX_NODES {
            return None;
        }
        self.nodes.push((
            id?,
            Node {
                object: Span {
                    start,
                    end: self.at,
                },
                children,
            },
        ));
        Some(())
    }
}
