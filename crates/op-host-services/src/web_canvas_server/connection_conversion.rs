//! Transfer large conversion bodies without changing borrowed dispatch users.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use super::{ConnCtx, Result};

// Conversion runs outside document locks. Without admission every connection
// could allocate a complete import graph simultaneously across all tenants.
static CONVERSION_ACTIVE: AtomicBool = AtomicBool::new(false);

struct ConversionLease<'a>(&'a AtomicBool);

impl<'a> ConversionLease<'a> {
    fn acquire(active: &'a AtomicBool) -> Option<Self> {
        active
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Self(active))
    }
}

impl Drop for ConversionLease<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub(super) fn serve_conversion<S: Write>(
    stream: &mut S,
    request: &crate::mcp_serve::HttpRequest,
    owned_body: Option<String>,
    cors_origin: Option<&str>,
) -> Result<bool> {
    serve_conversion_with_gate(stream, request, owned_body, cors_origin, &CONVERSION_ACTIVE)
}

fn serve_conversion_with_gate<S: Write>(
    stream: &mut S,
    request: &crate::mcp_serve::HttpRequest,
    owned_body: Option<String>,
    cors_origin: Option<&str>,
    active: &AtomicBool,
) -> Result<bool> {
    let Some(_conversion_lease) = ConversionLease::acquire(active) else {
        crate::mcp_serve::write_mcp_http_response_with_origin(
            stream,
            "503 Service Unavailable",
            &crate::mcp_serve::rest_error_body("another Figma conversion is active; retry later"),
            cors_origin,
        )?;
        return Ok(false);
    };
    let converted = match owned_body {
        Some(body) => crate::figma_convert::convert_fig_json_owned(body),
        None => crate::figma_convert::convert_fig_json(&request.body),
    };
    let (status, body) = match converted {
        Ok(body) => ("200 OK", body),
        Err(error) => (
            "400 Bad Request",
            serde_json::json!({ "ok": false, "error": error.to_string() }).to_string(),
        ),
    };
    // Hold admission while the response is buffered/written too: slow readers
    // must not accumulate document-sized converted responses concurrently.
    crate::mcp_serve::write_mcp_http_response_with_origin(stream, status, &body, cors_origin)?;
    Ok(false)
}

pub(in crate::web_canvas_server) fn dispatch<S: Read + Write>(
    stream: &mut S,
    request: &crate::mcp_serve::HttpRequest,
    context: &ConnCtx<'_>,
) -> Result<bool> {
    super::dispatch_inner(stream, request, context, None)
}

#[cfg(test)]
#[path = "connection_conversion_tests.rs"]
mod tests;

pub(in crate::web_canvas_server) fn dispatch_owned<S: Read + Write>(
    stream: &mut S,
    mut request: crate::mcp_serve::HttpRequest,
    context: &ConnCtx<'_>,
) -> Result<bool> {
    let conversion = (request.method == "POST" && request.path == "/api/figma/convert")
        .then(|| std::mem::take(&mut request.body));
    // Identity, origin, route and deployment checks still run in the common
    // dispatcher. Rejected requests simply drop the owned body here.
    match conversion {
        Some(body) => super::dispatch_inner(stream, &request, context, Some(body)),
        None => dispatch(stream, &request, context),
    }
}
