//! Independent canonical-file metadata cohorts. Each caller retains admission;
//! only a leader holds that file's editor lock through apply/save/accounting.
use super::{
    agent_context::{AgentAdmission, AgentContexts, AgentExecutionPermit},
    resident_documents::{
        resident_documents, ActionResult, AuthorityError, CanonicalPath, DocumentLease,
    },
    tool_profile::McpAccessProfile,
    McpServeError,
};
use crate::web_canvas_server::managed_group_commit::{budget, queue};
use op_editor_core::EditorCommand;
use std::{
    path::Path,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
#[path = "resident_group_metrics.rs"]
mod metrics;
#[path = "resident_group_quota.rs"]
mod quota;
#[path = "resident_group_transaction.rs"]
mod transaction;
const MAX_LINE: usize = 256 * 1024;
const MAX_REPLY: usize = 256 * 1024;

struct Request {
    claim_epoch: u64,
    line: String,
    profile: McpAccessProfile,
    contexts: Arc<AgentContexts>,
    permit: AgentExecutionPermit,
    retained: Arc<quota::Reservation>,
    route: RoutingGuard,
}
#[derive(Clone)]
pub(crate) struct RoutingGuard {
    recovery: crate::document_recovery::DocumentRecovery,
    epoch: u64,
}
impl RoutingGuard {
    pub(crate) fn new(recovery: &crate::document_recovery::DocumentRecovery, epoch: u64) -> Self {
        Self {
            recovery: recovery.clone(),
            epoch,
        }
    }
    // A stable fence on ANOTHER primary does not block independent files.
    // Only an observed routing/recovery transition invalidates this receipt.
    fn valid(&self) -> bool {
        self.recovery.epoch() == self.epoch
    }
}
enum ReplyValue {
    Complete(Result<String, McpServeError>),
    Isolated,
}
struct Reply {
    value: ReplyValue,
    // Shared reservation lives until the queued reply is actually consumed.
    _retained: Arc<quota::Reservation>,
}
impl Request {
    fn reply(&self, value: ReplyValue) -> Reply {
        Reply {
            value,
            _retained: Arc::clone(&self.retained),
        }
    }
    fn error(&self, message: &str) -> Reply {
        let response = op_mcp::parse_tool_call(&self.line)
            .map(|call| super::file_path::error_response(&call, message.to_owned()))
            .unwrap_or_default();
        self.reply(ReplyValue::Complete(Ok(response)))
    }
}
pub(super) struct Groups {
    queue: queue::Queue<Request, Reply>,
    metrics: metrics::Metrics,
}
impl Default for Groups {
    fn default() -> Self {
        Self {
            queue: queue::Queue::new(queue::Limits {
                calls: 1024,
                bytes: 512 * 1024 * 1024,
                group_calls: 256,
                group_bytes: 64 * 1024 * 1024,
                wait: Duration::from_millis(2),
            }),
            metrics: Default::default(),
        }
    }
}
impl Groups {
    pub(super) fn snapshot(&self) -> serde_json::Value {
        self.metrics.snapshot()
    }
}
fn candidate(line: &str, profile: McpAccessProfile) -> Option<op_mcp::ToolCall> {
    if profile.deny_unshareable_tools || line.len() > MAX_LINE || line.contains(['\n', '\r']) {
        return None;
    }
    let call = op_mcp::parse_tool_call(line)?;
    if !call.arguments.contains_key("agentId")
        || profile.refuse_call(&call.tool, &call.arguments).is_some()
    {
        return None;
    }
    if !super::cohort_coverage::tool(&call.tool) {
        return None;
    }
    Some(call)
}
fn audited_command(command: &EditorCommand) -> bool {
    super::cohort_coverage::command(command)
}

/// Called only by the already authenticated file router with its original agent
/// admission. None means no command was applied: the ordinary path stays valid.
pub(crate) fn dispatch(
    contexts: &Arc<AgentContexts>,
    admission: &AgentAdmission<'_>,
    current: Option<&Path>,
    line: &str,
    profile: McpAccessProfile,
    route: RoutingGuard,
) -> Option<Result<String, McpServeError>> {
    let call = candidate(line, profile)?;
    if !route.valid() {
        return Some(Ok(super::file_path::error_response(
            &call,
            "bound primary changed while routing this write; no command applied".into(),
        )));
    }
    let raw = call.arguments.get("filePath")?.trim();
    if raw.is_empty() || matches!(raw, "live://canvas" | "live://canvas/current") {
        return None;
    }
    let path = match CanonicalPath::resolve(Path::new(raw)) {
        Ok(path) => path,
        Err(error) => {
            return Some(Ok(super::file_path::error_response(
                &call,
                error.to_string(),
            )))
        }
    };
    if current.is_some_and(|current| {
        CanonicalPath::resolve(current).is_ok_and(|key| key.as_path() == path.as_path())
    }) {
        return None;
    }
    let lease = match resident_documents().acquire_canonical(&path) {
        Ok(lease) => lease,
        Err(error) => {
            return Some(Ok(super::file_path::error_response(
                &call,
                error.to_string(),
            )))
        }
    };
    let groups = lease.groups();
    let claim_epoch = match lease.claim_receipt() {
        Ok(epoch) => epoch,
        Err(error) => {
            return Some(Ok(super::file_path::error_response(
                &call,
                error.to_string(),
            )))
        }
    };
    let deferred = crate::web_canvas_server::async_completion::prepare();
    let charge = line
        .len()
        .saturating_add(if deferred.is_some() { line.len() } else { 0 })
        .saturating_add(MAX_REPLY)
        .saturating_add(lease.path().as_os_str().len() * 2)
        .saturating_add(4096);
    let Some(reserved) = quota::Reservation::queued(charge) else {
        groups.metrics.fallback.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    let permit = admission.execution_permit(contexts, lease.path())?;
    let input = Request {
        claim_epoch,
        line: line.to_owned(),
        profile,
        contexts: Arc::clone(contexts),
        permit,
        retained: Arc::new(reserved),
        route,
    };
    let deferred_pin = deferred.as_ref().map(|_| input.permit.clone());
    let deferred_line = deferred.as_ref().map(|_| input.line.clone());
    let deferred_route = deferred.as_ref().map(|_| input.route.clone());
    let ticket = match groups.queue.submit(input, line.len(), charge) {
        Ok(ticket) => ticket,
        Err(_) => {
            groups.metrics.fallback.fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };
    groups.metrics.admitted.fetch_add(1, Ordering::Relaxed);
    let fence = lease.recovery();
    if let Some(factory) = deferred {
        let run_lease = lease.clone();
        let fallback_lease = lease.clone();
        let panic_fence = fence.clone();
        let fallback_fence = fence.clone();
        let contexts = Arc::clone(contexts);
        let current = current.map(Path::to_path_buf);
        let line = deferred_line.expect("deferred resident request retains its input");
        let pin = deferred_pin.expect("deferred resident request retains admission");
        let route = deferred_route.expect("deferred resident request retains routing epoch");
        let pending = ticket.execute_async(
            move |requests| run(&run_lease, requests),
            move |request| {
                panic_fence.trip();
                request.reply(ReplyValue::Complete(Err(McpServeError::Io(
                    crate::document_recovery::REQUIRED.into(),
                ))))
            },
        );
        factory.install(async move {
            let (reply, queue_charge) = pending.await.into_parts();
            let Reply { value, _retained: global_charge } = reply;
            let body = match value {
                ReplyValue::Complete(reply) => reply?,
                ReplyValue::Isolated => {
                    let fallback_pin = pin.clone();
                    let unconfirmed_fence = fallback_fence.clone();
                    queue::execute_isolated(move || {
                        if fallback_fence.required() { return Err(McpServeError::Io(crate::document_recovery::REQUIRED.into())); }
                        if !route.valid() { return Err(McpServeError::Document("bound primary changed while this request waited; no command applied".into())); }
                        let _owner = fallback_lease.pin_receipt(claim_epoch).map_err(|error|McpServeError::Document(error.to_string()))?;
                        let _actor = super::agent_context::AgentDeliveryScope::enter(fallback_pin);
                        super::file_path::process_message_for_file_path_arg_with_contexts(&contexts, current.as_deref(), &line, profile)
                            .map(|reply|reply.unwrap_or_default())
                    }).await.map_err(|_| { unconfirmed_fence.trip(); crate::web_canvas_server_error::WebCanvasError::Config(crate::document_recovery::REQUIRED.into()) })??
                }
            };
            Ok(crate::web_canvas_server::async_completion::Response { body, _retained: Box::new((queue_charge,global_charge,pin)) })
        });
        return Some(Ok(String::new()));
    }
    let result = ticket.execute(
        |requests| run(&lease, requests),
        |request| {
            fence.trip();
            request.reply(ReplyValue::Complete(Err(McpServeError::Io(
                crate::document_recovery::REQUIRED.into(),
            ))))
        },
    );
    match result.value {
        ReplyValue::Complete(reply) => Some(reply),
        ReplyValue::Isolated if fence.required() => Some(Err(McpServeError::Io(
            crate::document_recovery::REQUIRED.into(),
        ))),
        ReplyValue::Isolated => None,
    }
}
fn run(lease: &DocumentLease<'_>, requests: &[Request]) -> Vec<Reply> {
    let groups = lease.groups();
    groups.metrics.cohorts.fetch_add(1, Ordering::Relaxed);
    groups.metrics.claimed[requests.len() - 1].fetch_add(1, Ordering::Relaxed);
    let claim_epoch = match lease.claim_receipt() {
        Ok(epoch) => epoch,
        Err(error) => {
            return requests
                .iter()
                .map(|request| request.error(&error.to_string()))
                .collect()
        }
    };
    let result = lease.with_tracked_editor_accounted_epoch(
        false,
        Some(claim_epoch),
        |tracked, stamp, fence| {
            let mut save_chain = tracked.take_save_chain();
            let mut tx = transaction::Transaction::new(
                tracked,
                lease.path(),
                stamp,
                fence,
                groups,
                requests,
                claim_epoch,
                &mut save_chain,
            );
            for index in 0..requests.len() {
                tx.stage(index);
            }
            let (replies, persisted, invalidate) = tx.finish();
            if !invalidate && !fence.required() {
                tracked.restore_save_chain(save_chain);
            }
            (
                ActionResult {
                    value: replies,
                    persisted,
                    invalidate,
                },
                None,
            )
        },
    );
    match result {
        Ok(replies) => replies,
        Err(AuthorityError::Uncertain) => requests
            .iter()
            .map(|request| {
                request.reply(ReplyValue::Complete(Err(McpServeError::Io(
                    crate::document_recovery::REQUIRED.into(),
                ))))
            })
            .collect(),
        Err(error) => requests
            .iter()
            .map(|request| request.error(&error.to_string()))
            .collect(),
    }
}

/// No filenames/account IDs are returned. The HTTP caller must restrict this
/// process-wide local-file diagnostic to Local/Managed, never Online tenants.
pub(crate) fn snapshot() -> serde_json::Value {
    serde_json::json!({"files":resident_documents().group_snapshots(),"globalQuota":quota::snapshot(),
        "queueCallsPerFile":1024,"cohortCallsPerFile":256,"cohortRequestBytes":64*1024*1024})
}
