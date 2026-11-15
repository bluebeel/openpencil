//! Shared JSON-RPC envelopes plus a small local TCP HTTP transport.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{IoSlice, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

pub const MCP_PATH: &str = "/mcp";
pub const POST_TIMEOUT: Duration = Duration::from_secs(60);
pub const PING_TIMEOUT: Duration = Duration::from_secs(2);
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: &str, params: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        }
    }

    /// Build an owned envelope without serializing and rebuilding `params`.
    pub fn into_value(self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("jsonrpc".into(), Value::String(self.jsonrpc.into()));
        object.insert("id".into(), self.id.into());
        object.insert("method".into(), Value::String(self.method));
        if !self.params.is_null() {
            object.insert("params".into(), self.params);
        }
        Value::Object(object)
    }
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn method_not_found(id: Value, method: &str) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("method '{method}' not supported"),
                data: None,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, PartialEq)]
pub enum Inbound {
    Response {
        id: u64,
        result: Option<Value>,
        error: Option<JsonRpcError>,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Unknown,
}

pub fn classify_inbound(value: &Value) -> Inbound {
    let obj = match value.as_object() {
        Some(o) => o,
        None => return Inbound::Unknown,
    };
    let has_method = obj.contains_key("method");
    let id = obj.get("id");
    match (has_method, id) {
        (true, Some(id)) if !id.is_null() => Inbound::Request {
            id: id.clone(),
            method: obj["method"].as_str().unwrap_or_default().to_string(),
            params: obj.get("params").cloned().unwrap_or(Value::Null),
        },
        (true, _) => Inbound::Notification {
            method: obj["method"].as_str().unwrap_or_default().to_string(),
            params: obj.get("params").cloned().unwrap_or(Value::Null),
        },
        (false, Some(id)) => match id.as_u64() {
            Some(id) => Inbound::Response {
                id,
                result: obj.get("result").cloned(),
                error: obj
                    .get("error")
                    .and_then(|e| JsonRpcError::deserialize(e).ok()),
            },
            None => Inbound::Unknown,
        },
        _ => Inbound::Unknown,
    }
}

/// Classify an already-owned frame while moving its potentially large payloads.
/// This has the same permissive classification rules as `classify_inbound`;
/// protocol-specific validation remains the caller's responsibility.
pub fn classify_inbound_owned(value: Value) -> Inbound {
    let Value::Object(mut object) = value else {
        return Inbound::Unknown;
    };
    if let Some(method) = object.remove("method") {
        let method = match method {
            Value::String(method) => method,
            _ => String::new(),
        };
        let params = object.remove("params").unwrap_or(Value::Null);
        return match object.remove("id") {
            Some(id) if !id.is_null() => Inbound::Request { id, method, params },
            _ => Inbound::Notification { method, params },
        };
    }
    let Some(id) = object.get("id").and_then(Value::as_u64) else {
        return Inbound::Unknown;
    };
    Inbound::Response {
        id,
        result: object.remove("result"),
        error: object
            .remove("error")
            .and_then(|error| serde_json::from_value(error).ok()),
    }
}

pub fn correlate(value: &Value, expected_id: u64) -> Option<Result<Value, JsonRpcError>> {
    // Check correlation before cloning a result: unrelated notifications and
    // responses can carry complete documents or image payloads.
    let object = value.as_object()?;
    if object.contains_key("method") || object.get("id").and_then(Value::as_u64)? != expected_id {
        return None;
    }
    Some(
        match object
            .get("error")
            .and_then(|error| JsonRpcError::deserialize(error).ok())
        {
            Some(error) => Err(error),
            None => Ok(object.get("result").cloned().unwrap_or(Value::Null)),
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpReply {
    pub status: u16,
    pub body: String,
}

/// Why a request over the small local TCP HTTP transport failed.
///
/// Structured per socket stage instead of a pre-formatted sentence.
/// `Display` reproduces the exact text [`TcpJsonRpc::send`] used to return as
/// `String`, so `op`'s `op: cannot reach the editor on …` output is unchanged
/// byte-for-byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpTransportError {
    /// `TcpStream::connect_timeout` to the local daemon failed or timed out.
    Connect {
        /// The loopback address that was dialed.
        addr: SocketAddr,
        /// The underlying `std::io::Error`, rendered.
        error: String,
    },
    /// Writing the HTTP request bytes onto the socket failed.
    Write(String),
    /// Reading the HTTP response bytes back off the socket failed.
    Read(String),
}

impl std::fmt::Display for HttpTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpTransportError::Connect { addr, error } => write!(
                f,
                "cannot reach the editor on {addr}: {error}\n\
                 start the OpenPencil MCP server and point clients at http://{addr}/mcp"
            ),
            HttpTransportError::Write(error) => write!(f, "http write: {error}"),
            HttpTransportError::Read(error) => write!(f, "http read: {error}"),
        }
    }
}

impl std::error::Error for HttpTransportError {}

/// Keeps `?` working for any caller that still collects transport failures as
/// `String`.
impl From<HttpTransportError> for String {
    fn from(error: HttpTransportError) -> String {
        error.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpJsonRpc {
    addr: SocketAddr,
    path: String,
    /// Per-instance token for the live MCP endpoint, sent as
    /// `X-OpenPencil-Token`. Empty means "send no header", which is what the
    /// endpoint's tokenless probes (`initialize`, `ping`) still accept.
    token: String,
}

impl TcpJsonRpc {
    pub fn localhost(port: u16, path: &str) -> Self {
        Self {
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
            path: path.to_string(),
            token: String::new(),
        }
    }

    pub fn local_mcp(port: u16) -> Self {
        Self::localhost(port, MCP_PATH)
    }

    /// Authenticates subsequent requests with the endpoint's instance token.
    ///
    /// The live MCP endpoint authenticates every stateful call, so a client
    /// that omits this gets `401` on anything beyond the discovery probes.
    #[must_use]
    pub fn with_token(mut self, token: &str) -> Self {
        self.token = token.to_string();
        self
    }

    fn http_post_header(&self, body_len: usize) -> String {
        use std::fmt::Write as _;
        let mut header = String::with_capacity(self.path.len() + self.token.len() + 160);
        let _ = write!(
            header,
            "POST {} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n",
            self.path
        );
        if !self.token.is_empty() {
            let _ = write!(header, "X-OpenPencil-Token: {}\r\n", self.token);
        }
        let _ = write!(
            header,
            "Content-Length: {body_len}\r\nConnection: close\r\n\r\n"
        );
        header
    }

    pub fn http_post_request(&self, body: &str) -> String {
        let mut request = self.http_post_header(body.len());
        request.push_str(body);
        request
    }

    pub fn http_get_request(&self, path: &str) -> String {
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
    }

    pub fn post_raw(&self, body: &str, timeout: Duration) -> Result<HttpReply, HttpTransportError> {
        self.send(&self.http_post_header(body.len()), body, timeout)
    }

    pub fn get_raw(&self, path: &str, timeout: Duration) -> Result<HttpReply, HttpTransportError> {
        self.send(&self.http_get_request(path), "", timeout)
    }

    fn send(
        &self,
        header: &str,
        body: &str,
        timeout: Duration,
    ) -> Result<HttpReply, HttpTransportError> {
        let mut stream = TcpStream::connect_timeout(&self.addr, timeout).map_err(|e| {
            HttpTransportError::Connect {
                addr: self.addr,
                error: e.to_string(),
            }
        })?;
        stream.set_read_timeout(Some(timeout)).ok();
        stream.set_write_timeout(Some(timeout)).ok();
        // Avoid a second allocation/copy of large JSON bodies while retaining
        // one socket write when the platform accepts both vectors together.
        let mut parts = [
            IoSlice::new(header.as_bytes()),
            IoSlice::new(body.as_bytes()),
        ];
        let mut remaining = &mut parts[..];
        while !remaining.is_empty() {
            match stream.write_vectored(remaining) {
                Ok(0) => {
                    return Err(HttpTransportError::Write(
                        std::io::Error::from(std::io::ErrorKind::WriteZero).to_string(),
                    ))
                }
                Ok(written) => IoSlice::advance_slices(&mut remaining, written),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(HttpTransportError::Write(error.to_string())),
            }
        }
        stream.flush().ok();
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .map_err(|e| HttpTransportError::Read(e.to_string()))?;
        Ok(split_http_response(response))
    }
}

fn split_http_response(mut response: String) -> HttpReply {
    let status = parse_status_code(&response).unwrap_or(0);
    let body_start = response.find("\r\n\r\n").map_or(0, |index| index + 4);
    let body = &response[body_start..];
    let leading = body.len() - body.trim_start().len();
    let end = body_start + leading + body.trim().len();
    response.truncate(end);
    response.drain(..body_start + leading);
    HttpReply {
        status,
        body: response,
    }
}

fn parse_status_code(response: &str) -> Option<u16> {
    let mut parts = response.lines().next()?.split_whitespace();
    let _http_version = parts.next()?;
    parts.next()?.parse::<u16>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn json_rpc_request_response_and_error_frames_are_canonical() {
        let req = JsonRpcRequest::new(7, "tools/list", Value::Null);
        let req_value = serde_json::to_value(&req).unwrap();
        assert_eq!(req_value["jsonrpc"], "2.0");
        assert_eq!(req_value["id"], 7);
        assert!(req_value.get("params").is_none());

        let ok = JsonRpcResponse::ok(json!(7), json!({ "pong": true }));
        let ok_value = serde_json::to_value(&ok).unwrap();
        assert_eq!(ok_value["result"]["pong"], true);
        assert!(ok_value.get("error").is_none());

        let err = JsonRpcResponse::method_not_found(json!("abc"), "missing");
        let err_value = serde_json::to_value(&err).unwrap();
        assert_eq!(err_value["id"], "abc");
        assert_eq!(err_value["error"]["code"], -32601);
    }

    #[test]
    fn classify_inbound_distinguishes_response_request_and_notification() {
        assert!(matches!(
            classify_inbound(&json!({"jsonrpc":"2.0","id":7,"result":{"ok":true}})),
            Inbound::Response { id: 7, .. }
        ));
        assert!(matches!(
            classify_inbound(
                &json!({"jsonrpc":"2.0","id":"abc","method":"client/request","params":{}})
            ),
            Inbound::Request { .. }
        ));
        assert!(matches!(
            classify_inbound(&json!({"jsonrpc":"2.0","method":"session/update","params":{}})),
            Inbound::Notification { .. }
        ));
        assert!(matches!(classify_inbound(&json!(false)), Inbound::Unknown));
    }

    #[test]
    fn correlate_returns_only_the_matching_response() {
        let response = json!({"jsonrpc":"2.0","id":42,"result":{"ok":true}});
        assert_eq!(correlate(&response, 42).unwrap().unwrap()["ok"], true);
        assert!(correlate(&response, 43).is_none());

        let error = json!({"jsonrpc":"2.0","id":42,"error":{"code":-32000,"message":"boom"}});
        let err = correlate(&error, 42).unwrap().unwrap_err();
        assert_eq!(err.code, -32000);
        assert_eq!(err.message, "boom");
    }

    #[test]
    fn tcp_http_request_targets_mcp_path() {
        let request = TcpJsonRpc::localhost(3100, MCP_PATH).http_post_request("{}");
        assert!(request.starts_with("POST /mcp HTTP/1.1\r\n"));
        assert!(request.contains("Host: 127.0.0.1\r\n"));
        assert!(request.contains("Content-Length: 2\r\n"));
    }
}
