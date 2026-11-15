//! REST dispatch separated from the document authority and startup spine.
use super::*;

pub fn handle_web_canvas_request(
    method: &str,
    path: &str,
    body: &str,
    state: &mut WebCanvasState,
) -> WebReply {
    if method == "POST" && path == "/api/file/reload" {
        return managed_group_commit::recovery::reload_reply(state);
    }
    if state.recovery.required() {
        return managed_group_commit::recovery::refusal();
    }
    let operation = state.recovery.watch();
    let reply = handle_healthy_request(method, path, body, state);
    operation.complete();
    reply
}

fn handle_healthy_request(
    method: &str,
    path: &str,
    body: &str,
    state: &mut WebCanvasState,
) -> WebReply {
    match (method, path) {
        ("GET", "/api/mcp/server") => WebReply {
            status: "200 OK",
            // `{running,port,localIp}` matches TS `server.get.ts`; the daemon
            // binds 127.0.0.1 (localhost-only) so localIp is loopback. Extra
            // `server`/`mode` fields are additive diagnostics.
            // `serveMode` is additive: the browser reads it to learn whether
            // the daemon is the sole sequencer for this document (online) or
            // merely a peer holding the operator's file (local/managed). That
            // decides whether a sync conflict may be auto-resolved — see
            // `op-host-web/src/live_sync_glue.rs::auto_resolve_is_safe`.
            // `fileBound`: a file backs this document (`--file` or a recent
            // open), so the browser opens onto its canvas rather than Home.
            // `siteImport`: `/api/ai/site-import` is served (Home offers it).
            // `variants`: `/api/ai/standard` runs side-by-side directions.
            // `generators`: `/api/generator/run` is served (every mode).
            body: format!(
                r#"{{"running":true,"port":{},"localIp":"127.0.0.1","server":"openpencil-mcp","mode":"web-canvas","serveMode":"{}","fileBound":{},"siteImport":{},"variants":{},"generators":{},"changefeed":{}}}"#,
                state.port,
                state.mode.wire_name(),
                state.current_path.is_some(),
                state.mode.allows_site_import(),
                state.mode.allows_design_variants(),
                crate::generator_run_route::SERVED,
                state.changefeed.stats()
            ),
        },
        ("POST", "/api/mcp/server") => update_mcp_server_settings(body, state),
        ("GET", "/api/mcp/document") => {
            // Cursor and complete snapshot are observed under the same state
            // lock; clients bootstrap once, then ask the bounded since API.
            let change_cursor = state.change_cursor();
            match serde_json::to_string(&state.editor.doc) {
                Ok(doc_json) => WebReply {
                    status: "200 OK",
                    body: format!(
                        r#"{{"document":{doc_json},"version":{},"activePageIndex":{},"preserveAuthoredGeometry":{},"scenario":{},"changeCursor":"{change_cursor}"}}"#,
                        state.version,
                        state.editor.ui.active_page_index,
                        state.editor.editor_ui.preserve_authored_geometry,
                        // The scene tag rides the same wire as the rest of the
                        // editor meta: a Slides deck must reach the browser as a
                        // deck or web preview cannot enter its presentation.
                        match state.editor.editor_ui.scenario {
                            Some(scene) => format!("\"{}\"", scene.as_str()),
                            None => "null".to_string(),
                        }
                    ),
                },
                Err(e) => WebReply {
                    status: "500 Internal Server Error",
                    body: crate::mcp_serve::rest_error_body(&e.to_string()),
                },
            }
        }
        ("POST", "/api/mcp/document") => match state.apply_document_push(body, None) {
            Ok(outcome) if outcome.applied => WebReply {
                status: "200 OK",
                body: crate::mcp_serve::document_sync_ok(outcome.current_version),
            },
            Ok(outcome) => WebReply {
                // Stale baseVersion: reject without writing, TS-style error
                // envelope plus the current version so the caller can decide
                // whether to refetch and retry.
                status: "409 Conflict",
                body: serde_json::json!({
                    "ok": false,
                    "error": "version-conflict",
                    "version": outcome.current_version,
                })
                .to_string(),
            },
            Err(error) => collab_aware_error_reply(&error),
        },
        ("GET", "/api/mcp/version") => {
            let change_cursor = state.change_cursor();
            WebReply {
                // `collabSeq` rides along so the existing 400 ms version poll also
                // notices collaboration changes without a second request. Adding a
                // field is backward compatible: a client that reads only `version`
                // is unaffected.
                status: "200 OK",
                body: format!(
                    r#"{{"version":{},"collabSeq":{},"changeCursor":"{change_cursor}"}}"#,
                    state.version,
                    state.collab.seq()
                ),
            }
        }
        ("GET", "/api/mcp/indicators") => WebReply {
            // Agent-indicator relay: design runs execute inside this
            // daemon, so the process-global registry the canvas paints
            // from lives HERE — the browser polls this and mirrors it
            // into its own registry (agent_indicators::apply_remote) so
            // agent borders / badges / reveal animations show on web.
            //
            // That registry has no tenant dimension, so a shared deployment
            // relays the empty projection instead of showing one account the
            // shape of another account's design run.
            status: "200 OK",
            body: if state.mode.allows_agent_indicator_relay() {
                op_editor_core::agent_indicators::relay_json()
            } else {
                online_policy::EMPTY_INDICATOR_RELAY.to_string()
            },
        },
        // The wasm shell posts a sync-reset on every mount. Locally that
        // means "the browser just booted, drop the transient document";
        // online it would wipe the document the returning account left
        // behind, so the route answers with the already-reset shape — the
        // same body a second local reset produces — and touches nothing.
        ("POST", "/api/mcp/sync-reset") if state.mode.sync_reset_is_noop() => WebReply {
            status: "200 OK",
            body: format!(
                r#"{{"ok":true,"skipped":true,"version":{}}}"#,
                state.version
            ),
        },
        ("POST", "/api/mcp/sync-reset") => match state.reset_document_guarded() {
            Ok(outcome) if outcome.skipped => WebReply {
                status: "200 OK",
                body: format!(
                    r#"{{"ok":true,"skipped":true,"version":{}}}"#,
                    state.version
                ),
            },
            Ok(_) => WebReply {
                status: "200 OK",
                body: crate::mcp_serve::document_sync_ok(state.version),
            },
            Err(e) if e.error_code().is_some() => collab_aware_error_reply(&e),
            Err(e) => WebReply {
                status: e.http_status(),
                body: crate::mcp_serve::rest_error_body(&format!("sync reset failed: {e}")),
            },
        },
        ("GET", "/api/mcp/selection") => {
            // TS `selection.get.ts` → `getSyncSelection()` shape:
            // `{selectedIds, activePageId}`. Read straight off the live
            // editor selection so MCP clients and the REST route agree.
            let ids: Vec<&str> = state
                .editor
                .selection
                .set
                .iter()
                .map(|id| id.as_str())
                .collect();
            let active_page_id = state
                .editor
                .doc
                .pages
                .as_ref()
                .and_then(|pages| pages.get(state.editor.ui.active_page_index))
                .map(|page| page.id.clone());
            let body = serde_json::json!({
                "selectedIds": ids,
                "activePageId": active_page_id,
            });
            WebReply {
                status: "200 OK",
                body: serde_json::to_string(&body)
                    .unwrap_or_else(|_| r#"{"selectedIds":[],"activePageId":null}"#.to_string()),
            }
        }
        ("POST", "/api/mcp/selection") => apply_selection_sync(body, state),
        // Both file routes touch the daemon host's filesystem, which in a
        // shared process means every account reading and writing through the
        // service account's paths. Refused before the handler, not inside it.
        ("POST", "/api/file/save" | "/api/file/open-recent")
            if !state.mode.allows_local_file_routes() =>
        {
            online_policy::refusal_reply(online_policy::OnlineRouteRefusal::LocalFileAccess)
        }
        // A save reply REPLACES `state.editor` (it installs the document it
        // just wrote, plus its metadata), so it is a whole-document swap and
        // has to clear the same gate `open-recent` does. Swapping the document
        // out from under a live session would leave the peers editing a
        // document this daemon no longer has.
        ("POST", "/api/file/save") => match state.gate_daemon_mutation(
            op_editor_core::CollabGateAction::ReplaceDocument,
            op_editor_core::CollabEditSource::ExternalSync,
        ) {
            Ok(()) => save_current_file(body, state),
            Err(refusal) => WebReply {
                status: refusal.http_status(),
                body: serde_json::json!({
                    "ok": false,
                    "error": refusal.code(),
                    "message": refusal.to_string(),
                })
                .to_string(),
            },
        },
        ("POST", "/api/file/open-recent") => open_recent_file(body, state),
        ("POST", "/api/file/unbind") => unbind_current_file(state),
        ("POST", "/api/export/pdf") => export_pdf_download(body, state),
        ("POST", "/api/export/raster") => export_raster_download(body, state),
        ("GET", "/api/ai/models") => WebReply {
            // JSON array of model ids the AI proxy can serve (the
            // configured built-in agents). The web bundle queries this
            // to populate its model picker without bundling a static
            // list or holding API keys. `POST /api/ai/stream` is a
            // streaming route handled in the connection loop, not here.
            status: "200 OK",
            body: crate::ai_proxy::models_json(&state.editor),
        },
        ("GET", "/api/settings/credential-policy") => WebReply {
            status: "200 OK",
            body: serde_json::json!({
                "serverPersistence": state.credential_persistence.server_persistence(),
            })
            .to_string(),
        },
        ("POST", "/api/settings/credentials") => {
            if !state.credential_persistence.server_persistence() {
                WebReply {
                    status: "403 Forbidden",
                    body: crate::mcp_serve::rest_error_body(
                        "server credential persistence is disabled",
                    ),
                }
            } else {
                match crate::web_credentials::apply_json(&mut state.editor, body) {
                    Ok(()) => WebReply {
                        status: "200 OK",
                        body: r#"{"ok":true}"#.into(),
                    },
                    // The oversize verdict now rides on the error itself
                    // (`is_payload_too_large`) instead of this route
                    // re-measuring the body against the cap — same 413/400
                    // split, one owner for the threshold.
                    Err(error) => WebReply {
                        status: if error.is_payload_too_large() {
                            "413 Payload Too Large"
                        } else {
                            "400 Bad Request"
                        },
                        body: crate::mcp_serve::rest_error_body(&error.to_string()),
                    },
                }
            }
        }
        // Device-login proxy: the wasm bundle ships no auth code and
        // drives the flow through the daemon's op-auth-bridge runtime.
        // (`POST /api/auth/login/begin` is a streaming-tier route — it
        // waits for the verification URI off the state lock.)
        //
        // The bridge holds ONE device session per process, so online the
        // routes fall through to the 404 arm: an account signs in to the
        // hub, not to this daemon, and proxying the service account's
        // session would sign every visitor in as it.
        (_, path) if !state.mode.allows_device_login_proxy() && is_device_login_route(path) => {
            not_found_reply()
        }
        ("GET", op_editor_core::auth_routes::STATUS) => crate::web_auth::status(state),
        ("GET", op_editor_core::auth_routes::LOGIN_STATUS) => crate::web_auth::login_status(state),
        ("POST", op_editor_core::auth_routes::LOGIN_CANCEL) => crate::web_auth::login_cancel(state),
        ("POST", op_editor_core::auth_routes::LOGOUT) => crate::web_auth::logout(state),
        // Collaboration: the runtime lives in this daemon, the panel in the
        // browser. See `web_canvas_server/collab_routes.rs`.
        ("GET", op_editor_core::collab_routes::STATE) => collab_routes::state(state),
        ("POST", op_editor_core::collab_routes::ACTION) => collab_routes::action(body, state),
        ("POST", op_editor_core::collab_routes::PRESENCE) => collab_routes::presence(body, state),
        _ => not_found_reply(),
    }
}

/// Whether `path` belongs to the daemon-hosted device-login proxy.
fn is_device_login_route(path: &str) -> bool {
    path.starts_with(op_editor_core::auth_routes::API_PREFIX)
}
