//! Manual JSON-RPC wire serializer for `ToolResponse` + internal
//! helpers. Hand-rolled so shell-core stays serde-free (wasm32
//! bundle size). Carved off `mcp.rs` to stay under the 800-line cap.

use op_util::json_escape::{push_escaped, push_json_quoted};
use std::{collections::BTreeMap, fmt::Write};

use super::{RequestId, ToolErrorCode, ToolResponse};

/// JSON-RPC wire serialiser for `ToolResponse`. Produces the
/// standard `{"jsonrpc": "2.0", "id": ..., "result": ...}` /
/// `{"jsonrpc": "2.0", "id": ..., "error": {"code": ..., "message":
/// ...}}` shape any MCP client expects.
pub fn response_to_json(r: &ToolResponse) -> String {
    let mut out = begin_response(r);
    match r {
        ToolResponse::Ok { result, json, .. } => {
            out.push_str(",\"result\":");
            match json {
                Some(raw) => out.push_str(raw),
                None => push_btree(&mut out, result),
            }
        }
        ToolResponse::Err { code, message, .. } => {
            write!(
                &mut out,
                ",\"error\":{{\"code\":{},\"message\":",
                error_code_to_int(*code)
            )
            .expect("writing to String is infallible");
            push_json_quoted(&mut out, message);
            out.push('}');
        }
    }
    out.push('}');
    out
}

/// JSON-RPC serializer for a *tool* result in the MCP-spec `tools/call`
/// shape, matching TS `pen-mcp` (`server.ts`): the tool's data rides inside
/// `result.content` as a single `text` block (the flat result map serialized
/// to a JSON string), and a tool-level failure becomes `isError:true` in the
/// result — NOT a JSON-RPC `error` (those are reserved for transport/parse
/// failures, still emitted via [`response_to_json`]). External MCP clients
/// (Claude Code / Codex) require this envelope.
///
/// When `image` is `Some`, an MCP `ImageContent` block is emitted BEFORE
/// any text block so vision-capable MCP clients receive the image as
/// multimodal content instead of a base64 string buried in JSON text.
pub fn tool_response_to_json(r: &ToolResponse) -> String {
    let mut out = begin_response(r);
    out.push_str(",\"result\":{\"content\":[");
    match r {
        ToolResponse::Ok {
            result,
            json,
            image,
            ..
        } => {
            if let Some(image) = image {
                out.push_str("{\"type\":\"image\",\"data\":");
                push_json_quoted(&mut out, &image.data);
                out.push_str(",\"mimeType\":");
                push_json_quoted(&mut out, &image.mime_type);
                out.push_str("},");
            }
            out.push_str("{\"type\":\"text\",\"text\":");
            match json {
                Some(raw) => push_json_quoted(&mut out, raw),
                None => push_json_quoted(&mut out, &btree_to_json(result)),
            }
            out.push_str("}]}}");
        }
        ToolResponse::Err { message, .. } => {
            out.push_str("{\"type\":\"text\",\"text\":\"Error: ");
            push_escaped(&mut out, message);
            out.push_str("\"}],\"isError\":true}}");
        }
    }
    out
}

fn begin_response(r: &ToolResponse) -> String {
    let (id, hint) = match r {
        ToolResponse::Ok {
            id, json, image, ..
        } => (
            id,
            json.as_ref()
                .map_or(0, String::len)
                .saturating_add(image.as_ref().map_or(0, |image| image.data.len())),
        ),
        ToolResponse::Err { id, message, .. } => (id, message.len()),
    };
    let mut out = String::with_capacity(hint.saturating_add(128));
    out.push_str("{\"jsonrpc\":\"2.0\",\"id\":");
    match id {
        RequestId::Str(id) => push_json_quoted(&mut out, id),
        RequestId::Num(id) => write!(&mut out, "{id}").expect("writing to String is infallible"),
    }
    out
}

pub(super) fn id_to_json(id: &RequestId) -> String {
    match id {
        RequestId::Str(s) => json_escape(s),
        RequestId::Num(n) => n.to_string(),
    }
}

pub(super) fn error_code_to_int(code: ToolErrorCode) -> i32 {
    // JSON-RPC reserves -32600..-32603 for transport-level errors;
    // tool errors live in the application range (-32000..-32099).
    match code {
        ToolErrorCode::MissingArgument => -32_001,
        ToolErrorCode::InvalidArgument => -32_602,
        ToolErrorCode::ToolFailed => -32_002,
        ToolErrorCode::UnknownTool => -32_601,
        ToolErrorCode::Internal => -32_603,
    }
}

pub(super) fn btree_to_json(m: &BTreeMap<String, String>) -> String {
    let mut out = String::from("{");
    push_btree_entries(&mut out, m);
    out.push('}');
    out
}

fn push_btree(out: &mut String, m: &BTreeMap<String, String>) {
    out.push('{');
    push_btree_entries(out, m);
    out.push('}');
}
fn push_btree_entries(out: &mut String, m: &BTreeMap<String, String>) {
    let mut first = true;
    for (key, value) in m {
        if !first {
            out.push(',');
        }
        first = false;
        push_json_quoted(out, key);
        out.push(':');
        push_json_quoted(out, value);
    }
}

/// Serialize `s` as a complete JSON string literal (quotes included).
/// Delegates to the canonical op-util escaper.
pub(super) fn json_escape(s: &str) -> String {
    op_util::json_escape::escape_json_quoted(s)
}
