//! Transfer large conversion bodies without changing borrowed dispatch users.

use std::io::{Read, Write};

use super::{ConnCtx, Result};

pub(in crate::web_canvas_server) fn dispatch<S: Read + Write>(
    stream: &mut S,
    request: &crate::mcp_serve::HttpRequest,
    context: &ConnCtx<'_>,
) -> Result<bool> {
    super::dispatch_inner(stream, request, context, None)
}

pub(in crate::web_canvas_server) fn dispatch_owned<S: Read + Write>(
    stream: &mut S,
    mut request: crate::mcp_serve::HttpRequest,
    context: &ConnCtx<'_>,
) -> Result<bool> {
    let conversion = (request.method == "POST" && request.path == "/api/figma/convert")
        .then(|| std::mem::take(&mut request.body));
    // Identity, origin, route and deployment checks still run in the common
    // dispatcher. Rejected requests simply drop the owned body here.
    super::dispatch_inner(stream, &request, context, conversion)
}
