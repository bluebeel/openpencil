//! Locate value spans in the immutable writer-produced node header. Payload
//! strings are skipped in buffered runs, and descendants are never scanned.
use super::{Node, Span};
use std::{
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
};
const MAX_KEYS: usize = 256 * 1024;
struct Scan<'a> {
    input: BufReader<&'a mut File>,
    at: usize,
    end: usize,
}
pub(super) fn locate(file: &mut File, node: Node) -> Option<Vec<(String, Span)>> {
    file.seek(SeekFrom::Start(node.object.start.try_into().ok()?))
        .ok()?;
    let mut scan = Scan {
        input: BufReader::with_capacity(16 * 1024, file),
        at: node.object.start,
        end: node.object.end,
    };
    scan.expect(b'{')?;
    let mut fields = Vec::new();
    let mut key_bytes = 0usize;
    scan.space()?;
    if scan.peek()? != b'}' {
        loop {
            if fields.len() == 4096 {
                return None;
            }
            let key: String = serde_json::from_slice(&scan.string(true)?).ok()?;
            key_bytes = key_bytes.checked_add(key.capacity())?;
            if key_bytes > MAX_KEYS || fields.iter().any(|(old, _)| old == &key) {
                return None;
            }
            scan.expect(b':')?;
            scan.space()?;
            let start = scan.at;
            if key == "children" {
                let children = node.children?;
                if children.span.start != start || children.span.end > scan.end {
                    return None;
                }
                scan.input
                    .seek(SeekFrom::Start(children.span.end.try_into().ok()?))
                    .ok()?;
                scan.at = children.span.end;
            } else {
                scan.value()?;
            }
            fields.push((
                key,
                Span {
                    start,
                    end: scan.at,
                },
            ));
            scan.space()?;
            match scan.byte()? {
                b',' => (),
                b'}' => return (scan.at == scan.end).then_some(fields),
                _ => return None,
            }
        }
    }
    scan.expect(b'}')?;
    (scan.at == scan.end).then_some(fields)
}
impl Scan<'_> {
    fn peek(&mut self) -> Option<u8> {
        (self.at < self.end).then_some(())?;
        self.input.fill_buf().ok()?.first().copied()
    }
    fn advance(&mut self, n: usize) -> Option<()> {
        self.at = self.at.checked_add(n)?;
        if self.at > self.end {
            return None;
        }
        self.input.consume(n);
        Some(())
    }
    fn byte(&mut self) -> Option<u8> {
        let value = self.peek()?;
        self.advance(1)?;
        Some(value)
    }
    fn space(&mut self) -> Option<()> {
        loop {
            let remaining = self.end.checked_sub(self.at)?;
            let bytes = self.input.fill_buf().ok()?;
            let n = bytes
                .iter()
                .take(remaining)
                .take_while(|b| b.is_ascii_whitespace())
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
    fn string(&mut self, capture: bool) -> Option<Vec<u8>> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        if capture {
            out.push(b'"');
        }
        loop {
            let remaining = self.end.checked_sub(self.at)?;
            let bytes = self.input.fill_buf().ok()?;
            let bytes = &bytes[..bytes.len().min(remaining)];
            if bytes.is_empty() {
                return None;
            }
            let n = bytes
                .iter()
                .position(|b| matches!(b, b'"' | b'\\'))
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
                        if out.len().checked_add(2)? > 16 * 1024 {
                            return None;
                        }
                        out.extend_from_slice(&[b'\\', escaped]);
                    }
                }
                _ => return None,
            }
        }
    }
    fn value(&mut self) -> Option<()> {
        self.space()?;
        match self.peek()? {
            b'"' => {
                self.string(false)?;
            }
            b'{' | b'[' => {
                let mut stack = Vec::new();
                loop {
                    match self.peek()? {
                        b'"' => {
                            self.string(false)?;
                            continue;
                        }
                        b'{' => stack.push(b'}'),
                        b'[' => stack.push(b']'),
                        b'}' | b']' => {
                            if stack.pop()? != self.peek()? {
                                return None;
                            }
                        }
                        _ => (),
                    }
                    if stack.len() > 512 {
                        return None;
                    }
                    self.advance(1)?;
                    if stack.is_empty() {
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
                if start == self.at {
                    return None;
                }
            }
        }
        Some(())
    }
}
