//! Writer-independent counters; no document, resident registry or file access.
use super::*;

pub(super) fn serve<S: Read + Write>(
    stream: &mut S,
    ctx: &ConnCtx<'_>,
    cors_origin: Option<&str>,
) -> Result<()> {
    let mut body = serde_json::json!({
        "ok": true,
        "serveMode": ctx.mode.wire_name(),
        "consistency": "independent atomic counters",
        "durableGroups": ctx.authority.managed_groups.metrics.compact_snapshot(),
    });
    if !ctx.mode.is_online() {
        body["incrementalSaves"] = crate::doc_io::incremental::snapshot_stats();
        body["incrementalSaveScope"] = "local process".into();
    }
    crate::mcp_serve::write_mcp_http_response_with_origin(
        stream,
        "200 OK",
        &body.to_string(),
        cors_origin,
    )
    .map_err(Into::into)
}
