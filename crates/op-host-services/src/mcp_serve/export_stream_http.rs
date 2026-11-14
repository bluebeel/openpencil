//! A fixed-capacity HTTP/1.1 chunk writer. Content is the same complete JSON
//! result as inline delivery; no progress protocol or artifact files are added.
use std::io::{self, Write};
const CHUNK: usize = 8192;
pub(super) struct Chunks<'a, W> {
    output: &'a mut W,
    buffer: [u8; CHUNK],
    used: usize,
}
impl<'a, W: Write> Chunks<'a, W> {
    pub(super) fn start(output: &'a mut W, origin: Option<&str>) -> io::Result<Self> {
        let cors = origin
            .map(|origin| format!("Access-Control-Allow-Origin: {origin}\r\n"))
            .unwrap_or_default();
        write!(
            output,
            "HTTP/1.1 200 OK\r\n{cors}\
Access-Control-Allow-Methods: GET, POST, DELETE, OPTIONS\r\n\
Access-Control-Allow-Headers: Content-Type, mcp-session-id, X-OpenPencil-Token, Authorization\r\n\
Access-Control-Expose-Headers: mcp-session-id\r\n\
mcp-session-id: openpencil\r\nCache-Control: no-store\r\n\
Content-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        )?;
        Ok(Self {
            output,
            buffer: [0; CHUNK],
            used: 0,
        })
    }
    fn emit(&mut self) -> io::Result<()> {
        if self.used != 0 {
            write!(self.output, "{:x}\r\n", self.used)?;
            self.output.write_all(&self.buffer[..self.used])?;
            self.output.write_all(b"\r\n")?;
            self.used = 0;
        }
        Ok(())
    }
    pub(super) fn finish(mut self) -> io::Result<()> {
        self.emit()?;
        self.output.write_all(b"0\r\n\r\n")?;
        self.output.flush()
    }
}
impl<W: Write> Write for Chunks<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let n = remaining.len().min(CHUNK - self.used);
            self.buffer[self.used..self.used + n].copy_from_slice(&remaining[..n]);
            self.used += n;
            remaining = &remaining[n..];
            if self.used == CHUNK {
                self.emit()?;
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.emit()?;
        self.output.flush()
    }
}
