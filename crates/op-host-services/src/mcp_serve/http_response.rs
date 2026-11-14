//! MCP HTTP response writing.

use super::McpServeError;
use std::io::{IoSlice, Write};

pub(crate) fn write_mcp_http_response_with_origin<S: std::io::Write>(
    stream: &mut S,
    status: &str,
    body: &str,
    cors_origin: Option<&str>,
) -> Result<(), McpServeError> {
    let resolved = super::http_export::complete_response(body)?;
    let body = resolved.as_deref().unwrap_or(body);
    if let Some(result) = super::export_stream::try_write(stream, status, body, cors_origin) {
        if result.is_err() {
            super::http_export::cancel_current();
        }
        return result;
    }
    let cors_line = cors_origin
        .map(|origin| format!("Access-Control-Allow-Origin: {origin}\r\n"))
        .unwrap_or_default();
    let header = format!(
        "HTTP/1.1 {status}\r\n\
         {cors_line}\
         Access-Control-Allow-Methods: GET, POST, DELETE, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type, mcp-session-id, X-OpenPencil-Token, Authorization\r\n\
         Access-Control-Expose-Headers: mcp-session-id\r\n\
         mcp-session-id: openpencil\r\n\
         Cache-Control: no-store\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    write_parts(stream, header.as_bytes(), body.as_bytes())
        .map_err(|e| McpServeError::Io(format!("http write: {e}")))?;
    stream
        .flush()
        .map_err(|e| McpServeError::Io(format!("http flush: {e}")))
}

// TcpStream sends header and borrowed body together with writev. This avoids
// a payload-sized copy without adding a tiny header-only TCP packet/delayed ACK.
fn write_parts<S: Write>(stream: &mut S, header: &[u8], body: &[u8]) -> std::io::Result<()> {
    let mut parts = [IoSlice::new(header), IoSlice::new(body)];
    let mut remaining = &mut parts[..];
    while !remaining.is_empty() {
        match stream.write_vectored(remaining) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(written) => IoSlice::advance_slices(&mut remaining, written),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "http_response_tests.rs"]
mod tests;
