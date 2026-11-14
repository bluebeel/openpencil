//! Since-cursor reads use the normal Origin/scope/tenant gates. Responses borrow
//! shared encoded records after releasing the writer mutex; no document copy.

use super::{Batch, MAX_RESPONSE_RECORDS};
use crate::web_canvas_server::{connection::ConnCtx, Result, WebCanvasError};
use std::io::{IoSlice, Write};

pub(in crate::web_canvas_server) fn serve<S: Write>(
    stream: &mut S,
    request: &crate::mcp_serve::HttpRequest,
    ctx: &ConnCtx<'_>,
    cors_origin: Option<&str>,
) -> Result<()> {
    let (cursor, limit) = match arguments(request.query.as_deref()) {
        Some(arguments) => arguments,
        None => {
            crate::mcp_serve::write_mcp_http_response_with_origin(
                stream,
                "400 Bad Request",
                r#"{"ok":false,"error":"invalid-change-cursor","message":"use one since cursor and optional limit from1to64"}"#,
                cors_origin,
            )?;
            return Ok(());
        }
    };
    let batch = {
        let mut state = ctx
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if state.recovery.required() {
            let reply = crate::web_canvas_server::managed_group_commit::recovery::refusal();
            drop(state);
            crate::mcp_serve::write_mcp_http_response_with_origin(
                stream,
                reply.status,
                &reply.body,
                cors_origin,
            )?;
            return Ok(());
        }
        let current = state.change_cursor();
        match cursor {
            Some(cursor) => state.changefeed.since(&cursor, limit),
            None => Batch::Reset {
                cursor: current,
                version: state.version,
                reason: "snapshot-required",
            },
        }
    };
    write_batch(stream, &batch, cors_origin)
}

fn arguments(query: Option<&str>) -> Option<(Option<String>, usize)> {
    let mut since = None;
    let mut limit = None;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        match key.as_ref() {
            "since" if since.is_none() && !value.is_empty() && value.len() <= 128 => {
                since = Some(value.into_owned())
            }
            "limit" if limit.is_none() => {
                limit = Some(
                    value
                        .parse::<usize>()
                        .ok()
                        .filter(|value| (1..=MAX_RESPONSE_RECORDS).contains(value))?,
                )
            }
            // The online dispatcher has already resolved and authorized this
            // account; it is not a cursor-selected document authority.
            "tenant" => {}
            _ => return None,
        }
    }
    Some((since, limit.unwrap_or(MAX_RESPONSE_RECORDS)))
}

fn write_batch<S: Write>(stream: &mut S, batch: &Batch, origin: Option<&str>) -> Result<()> {
    let (prefix, suffix, records) = match batch {
        Batch::Reset { cursor, version, reason } => {
            let body = serde_json::json!({"ok":false,"schema":1,"resyncRequired":true,"reason":reason,"cursor":cursor,"version":version,"snapshotRoute":"/api/mcp/document","replay":false}).to_string();
            crate::mcp_serve::write_mcp_http_response_with_origin(stream, "409 Conflict", &body, origin)?;
            return Ok(());
        }
        Batch::Data { from, next, version, head, more, records } => (
            format!("{{\"ok\":true,\"schema\":1,\"resyncRequired\":false,\"fromVersion\":{from},\"version\":{version},\"headVersion\":{head},\"cursor\":\"{next}\",\"more\":{more},\"changes\":["),
            b"]}".as_slice(), records,
        ),
    };
    let bytes = prefix.len()
        + suffix.len()
        + records
            .iter()
            .map(|record| record.encoded.len())
            .sum::<usize>()
        + records.len().saturating_sub(1);
    let cors = origin
        .map(|origin| format!("Access-Control-Allow-Origin: {origin}\r\n"))
        .unwrap_or_default();
    let header = format!("HTTP/1.1 200 OK\r\n{cors}Access-Control-Allow-Methods: GET, POST, DELETE, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, mcp-session-id, X-OpenPencil-Token, Authorization\r\nAccess-Control-Expose-Headers: mcp-session-id\r\nmcp-session-id: openpencil\r\nCache-Control: no-store\r\nContent-Type: application/json\r\nContent-Length: {bytes}\r\nConnection: close\r\n\r\n");
    let mut parts = Vec::with_capacity(records.len() * 2 + 3);
    parts.push(IoSlice::new(header.as_bytes()));
    parts.push(IoSlice::new(prefix.as_bytes()));
    for (index, record) in records.iter().enumerate() {
        if index != 0 {
            parts.push(IoSlice::new(b","));
        }
        parts.push(IoSlice::new(&record.encoded));
    }
    parts.push(IoSlice::new(suffix));
    let mut remaining = parts.as_mut_slice();
    while !remaining.is_empty() {
        match stream.write_vectored(remaining) {
            Ok(0) => {
                return Err(WebCanvasError::Transport(
                    "changefeed response: write zero".into(),
                ))
            }
            Ok(written) => IoSlice::advance_slices(&mut remaining, written),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(WebCanvasError::Transport(format!(
                    "changefeed response: {error}"
                )))
            }
        }
    }
    stream
        .flush()
        .map_err(|error| WebCanvasError::Transport(format!("changefeed flush: {error}")))
}
