//! MCP HTTP response writing.

use super::McpServeError;

pub(crate) fn write_mcp_http_response_with_origin<S: std::io::Write>(
    stream: &mut S,
    status: &str,
    body: &str,
    cors_origin: Option<&str>,
) -> Result<(), McpServeError> {
    let cors_line = cors_origin
        .map(|origin| format!("Access-Control-Allow-Origin: {origin}\r\n"))
        .unwrap_or_default();
    let http = format!(
        "HTTP/1.1 {status}\r\n\
         {cors_line}\
         Access-Control-Allow-Methods: GET, POST, DELETE, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type, mcp-session-id, X-OpenPencil-Token, Authorization\r\n\
         Access-Control-Expose-Headers: mcp-session-id\r\n\
         mcp-session-id: openpencil\r\n\
         Cache-Control: no-store\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(http.as_bytes())
        .map_err(|e| McpServeError::Io(format!("http write: {e}")))?;
    stream
        .flush()
        .map_err(|e| McpServeError::Io(format!("http flush: {e}")))
}
