//! Shared online preflight and authorized dispatch for sync and queued sockets.
use super::super::tenant::TenantError;
use super::super::tenant_auth::ResolvedIdentity;
use super::*;
use crate::mcp_serve::{HttpBodyLease, HttpRequest};

pub(super) enum Prepared {
    Complete(bool),
    Ready(Authorized),
}
pub(super) struct Authorized {
    pub(super) owner: String,
    pub(super) req: HttpRequest,
    identity: ResolvedIdentity,
    _body_lease: HttpBodyLease<'static>,
}
impl Authorized {
    pub(super) fn queue_bytes(&self) -> usize {
        // Retained request strings/callback/socket/identity plus queue metadata.
        let strings = [
            Some(&self.req.method),
            Some(&self.req.path),
            Some(&self.req.body),
            self.req.host.as_ref(),
            self.req.origin.as_ref(),
            self.req.token.as_ref(),
            self.req.content_type.as_ref(),
            self.req.authorization.as_ref(),
            self.req.cookie.as_ref(),
            self.req.query.as_ref(),
        ];
        strings
            .into_iter()
            .flatten()
            .fold(2048usize, |n, s| n.saturating_add(s.capacity()))
            .saturating_add(self.owner.capacity())
            .saturating_add(self.identity.user_id.capacity())
            .saturating_add(self.identity.username.capacity())
            .saturating_add(self.identity.display_name.capacity())
    }
}

pub(super) fn prepare<S: Read + Write>(
    stream: &mut S,
    registry: &TenantRegistry,
    verifier: &dyn IdentityVerifier,
) -> Result<Prepared> {
    let Some((req, body_lease)) = crate::mcp_serve::read_admitted_http_request(stream)? else {
        return Ok(Prepared::Complete(false));
    };
    let allow_origins = registry.allow_origins();
    let cors_origin = online_policy::online_cors_origin(allow_origins, req.origin.as_deref());
    if let Some(done) = serve_anonymous_prefix(stream, &req, cors_origin.as_deref())? {
        return Ok(Prepared::Complete(done));
    }
    // The tenant key comes from here and nowhere else. Note that the request
    // body has not been looked at yet, and never contributes.
    let identity = match verifier.resolve(&PresentedCredentials::from_request(&req)) {
        Ok(identity) => identity,
        Err(error) => {
            crate::mcp_serve::write_mcp_http_response(
                stream,
                error.http_status(),
                &serde_json::json!({
                    "ok": false,
                    "error": error.code(),
                    "message": error.to_string(),
                })
                .to_string(),
            )?;
            return Ok(Prepared::Complete(false));
        }
    };
    // CSRF boundary. A session cookie rides along on any cross-site request
    // the browser makes, so on a public origin the cookie alone does not
    // establish that this deployment's own page initiated the write. A bearer
    // token is exempt: it is only ever attached by code that already holds it.
    if identity.via == super::super::tenant_auth::IdentityVia::SessionCookie
        && !matches!(req.method.as_str(), "GET" | "HEAD" | "OPTIONS")
        && !online_policy::cookie_write_origin_allowed(allow_origins, req.origin.as_deref())
    {
        crate::mcp_serve::write_mcp_http_response_with_origin(
            stream,
            "403 Forbidden",
            &serde_json::json!({
                "ok": false,
                "error": "cross-origin-write-forbidden",
                "message": "a cookie-authenticated write must come from this deployment's origin",
            })
            .to_string(),
            cors_origin.as_deref(),
        )?;
        return Ok(Prepared::Complete(false));
    }
    // Admission is keyed only after verification and a live/persisted share ACL
    // check. No tenant document is materialized or pinned just to queue a call.
    let requested = req
        .query
        .as_deref()
        .and_then(op_editor_core::share_routes::tenant_from_query);
    let owner = match registry.queued_owner(&identity, requested) {
        Ok(owner) => owner,
        Err(error) => {
            tenant_error(stream, error)?;
            return Ok(Prepared::Complete(false));
        }
    };
    Ok(Prepared::Ready(Authorized {
        owner,
        req,
        identity,
        _body_lease: body_lease,
    }))
}

pub(super) fn dispatch<S: Read + Write>(
    stream: &mut S,
    authorized: Authorized,
    registry: &TenantRegistry,
    write_barrier: &super::super::tenant::WriteBarrier,
) -> Result<bool> {
    let Authorized {
        req,
        identity,
        _body_lease,
        ..
    } = authorized;
    let cors_origin =
        online_policy::online_cors_origin(registry.allow_origins(), req.origin.as_deref());
    // Which tenant is this request addressed to? The query names an OWNER;
    // whether this caller may reach it is still decided by the owner's access
    // list, never by the parameter. Absent, it is the caller's own tenant.
    let requested_owner = req
        .query
        .as_deref()
        .and_then(op_editor_core::share_routes::tenant_from_query)
        .map(str::to_string);
    let lease = match requested_owner.as_deref() {
        Some(owner) => registry.lease_for_shared(owner, &identity),
        None => registry.lease_for(&identity),
    };
    let lease = match lease {
        Ok(lease) => lease,
        Err(error) => {
            crate::mcp_serve::write_mcp_http_response(
                stream,
                error.http_status(),
                &serde_json::json!({
                    "ok": false,
                    "error": error.code(),
                    "message": error.to_string(),
                })
                .to_string(),
            )?;
            return Ok(false);
        }
    };
    // Sharing is administered on the CALLER's own tenant, so it is answered
    // here rather than in the document tier — a visitor holding a `?tenant=`
    // lease on someone else's document must not be able to re-share it.
    if super::super::share_routes::is_share_route(&req.path) {
        // Same gate the connection tier applies, because this route is
        // dispatched before it ever runs: a grant is a write.
        if let Some(refusal) = super::super::tool_scopes::check_rest_scope(
            identity.via,
            identity.scopes,
            &req.method,
            &req.path,
        ) {
            crate::mcp_serve::write_mcp_http_response_with_origin(
                stream,
                refusal.http_status(),
                &serde_json::json!({
                    "ok": false,
                    "error": refusal.code(),
                    "message": refusal.to_string(),
                })
                .to_string(),
                cors_origin.as_deref(),
            )?;
            return Ok(false);
        }
        let own = match registry.lease_for(&identity) {
            Ok(own) => own,
            Err(error) => {
                crate::mcp_serve::write_mcp_http_response_with_origin(
                    stream,
                    error.http_status(),
                    &serde_json::json!({
                        "ok": false,
                        "error": error.code(),
                        "message": error.to_string(),
                    })
                    .to_string(),
                    cors_origin.as_deref(),
                )?;
                return Ok(false);
            }
        };
        let reply = super::super::share_routes::handle(
            &req.method,
            &req.path,
            &req.body,
            &identity,
            &own,
            registry,
        );
        crate::mcp_serve::write_mcp_http_response_with_origin(
            stream,
            reply.status,
            &reply.body,
            cors_origin.as_deref(),
        )?;
        return Ok(false);
    }
    // Synchronous callers hold the lease throughout dispatch. Production SSE
    // moves this exact lease into its socket task after authorized dispatch.
    let outcome = dispatch_owned(
        stream,
        req,
        &ConnCtx {
            state: lease.state(),
            authority: &lease.tenant().authority,
            hub: lease.hub(),
            mode: ServeMode::Online,
            // The public tool profile, narrowed further by whatever scopes
            // this particular credential carries.
            mcp_profile: crate::mcp_serve::tool_profile::McpAccessProfile::online(identity.scopes),
            rest_identity: Some(identity.clone()),
            write_barrier: Some(write_barrier),
        },
    );
    if outcome.is_ok() {
        async_sse::retain_tenant(lease);
    }
    outcome
}

fn tenant_error<S: Write>(stream: &mut S, error: TenantError) -> Result<()> {
    crate::mcp_serve::write_mcp_http_response(
        stream,
        error.http_status(),
        &serde_json::json!({"ok":false,"error":error.code(),"message":error.to_string()})
            .to_string(),
    )?;
    Ok(())
}
