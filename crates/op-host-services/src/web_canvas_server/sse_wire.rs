//! The exact wire format shared by synchronous compatibility and async sockets.

use super::SseTick;

pub(super) fn headers(cors_origin: Option<&str>) -> String {
    let cors_line = cors_origin
        .map(|origin| format!("Access-Control-Allow-Origin: {origin}\r\n"))
        .unwrap_or_default();
    format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream\r\n\
         Cache-Control: no-cache\r\n\
         Connection: keep-alive\r\n\
         {cors_line}\r\n"
    )
}

pub(super) fn event(tick: SseTick) -> String {
    format!(
        "data: {{\"version\":{},\"collabSeq\":{}}}\n\n",
        tick.version, tick.collab_seq
    )
}
