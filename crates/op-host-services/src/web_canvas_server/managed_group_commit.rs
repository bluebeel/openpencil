//! Coalesced primary Managed saves for audited, admitted nonstructural setters.
//! Every command keeps its own undo entry. Unsupported work never applies here.
use super::{connection::ConnCtx, ServeMode, WebCanvasState};
use crate::{mcp_serve, web_canvas_server_error::WebCanvasError};
use mcp_serve::{
    agent_context::{AgentAdmission, AgentExecutionPermit},
    tool_profile::McpAccessProfile,
};
use op_editor_core::{CommandEditCapture, EditorCommand as C};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
#[path = "document_recovery.rs"]
pub(super) mod recovery;
use std::sync::atomic::Ordering;
#[path = "managed_group_budget.rs"]
mod budget;
#[path = "managed_group_metrics.rs"]
mod metrics;
#[path = "managed_group_queue.rs"]
mod queue;

const MAX_BODY: usize = 16 * 1024 * 1024;
const MAX_REPLY: usize = 256 * 1024;

struct Request {
    line: String,
    profile: McpAccessProfile,
    path: PathBuf,
    document: PathBuf,
    state_identity: usize,
    recovery_epoch: u64,
    generation: u64,
    permit: AgentExecutionPermit,
}

enum Reply {
    Complete(Result<String, WebCanvasError>),
    /// No command was applied, so the original worker can use its old path.
    Isolated,
}

pub(super) struct Groups {
    queue: queue::Queue<Request, Reply>,
    pub(in crate::web_canvas_server) metrics: metrics::Metrics,
    #[cfg(test)]
    saves: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    fault: std::sync::atomic::AtomicU8,
}

impl Default for Groups {
    fn default() -> Self {
        Self {
            queue: queue::Queue::new(queue::Limits {
                calls: 128,
                bytes: 128 * 1024 * 1024,
                group_calls: 32,
                group_bytes: 32 * 1024 * 1024,
                wait: Duration::from_millis(2),
            }),
            metrics: Default::default(),
            #[cfg(test)]
            saves: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            fault: std::sync::atomic::AtomicU8::new(0),
        }
    }
}

#[derive(Debug)]
enum GroupError {
    AuthorityChanged,
    CommandPanicked,
    CommandCount,
    PartialRefusal,
    Save(crate::doc_io::DocIoError),
    ContextDidNotSave,
    Rollback(op_editor_core::CommandEditError),
    RollbackContext,
    Unavailable,
    LeaderPanicked,
}
impl std::fmt::Display for GroupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AuthorityChanged => f.write_str("managed primary changed while routing this write; no command was applied"),
            Self::CommandPanicked => f.write_str("managed grouped command panicked; command rolled back"),
            Self::CommandCount => f.write_str("managed grouped setter emitted more than one command; command rolled back"),
            Self::PartialRefusal => f.write_str("managed grouped setter refused after changing the document; command rolled back"),
            Self::Save(source) => write!(f, "managed primary save failed: {source}; command rolled back"),
            Self::ContextDidNotSave => f.write_str("managed group could not save in the final writer's admitted context; no successful write acknowledged"),
            Self::Rollback(error) => write!(f, "managed grouped rollback failed: {error}; authority requires recovery"),
            Self::RollbackContext => f.write_str("managed grouped rollback lost an admitted actor context; authority requires recovery"),
            Self::Unavailable => f.write_str("document-recovery-required: explicitly reload the authoritative saved file before accessing this document; do not replay the failed request"),
            Self::LeaderPanicked => f.write_str("managed group outcome is unconfirmed after internal failure; inspect persisted state before retrying"),
        }
    }
}
impl std::error::Error for GroupError {}

fn error(line: &str, reason: &impl std::fmt::Display) -> String {
    op_mcp::parse_tool_call(line)
        .map(|call| mcp_serve::file_path::error_response(&call, reason.to_string()))
        .unwrap_or_default()
}

fn candidate(line: &str, profile: McpAccessProfile) -> bool {
    if line.len() > MAX_BODY || line.contains(['\n', '\r']) {
        return false;
    }
    if !serde_json::from_str::<serde_json::Value>(line).is_ok_and(|value| value.is_object()) {
        return false;
    }
    let Some(call) = op_mcp::parse_tool_call(line) else {
        return false;
    };
    if !call.arguments.contains_key("agentId")
        || profile.refuse_call(&call.tool, &call.arguments).is_some()
    {
        return false;
    }
    matches!(
        call.tool.as_str(),
        "update_node"
            | "set_node_rotation"
            | "set_node_text"
            | "set_node_corner_radius"
            | "set_node_font_size"
            | "set_node_font_weight"
            | "set_node_stroke_hex"
            | "set_node_stroke_width"
            | "set_node_fill_hex"
            | "set_node_name"
            | "set_node_hidden"
            | "set_node_locked"
            | "set_node_collapsed"
            | "set_node_flip"
            | "set_ellipse_arc"
            | "add_node_effect"
            | "remove_node_effect"
            | "set_node_stroke_side_width"
            | "insert_node"
            | "delete_node"
            | "copy_node"
    )
}

fn audited(command: &C) -> bool {
    match command {
        C::UpdateNode { .. }
        | C::SetNodeRotation { .. }
        | C::SetNodeText { .. }
        | C::SetNodeCornerRadius { .. }
        | C::SetNodeFontSize { .. }
        | C::SetNodeFontWeight { .. }
        | C::SetNodeStrokeHex { .. }
        | C::SetNodeStrokeWidth { .. }
        | C::SetNodeFillHex { .. }
        | C::SetNodeName { .. }
        | C::SetNodeFlag { .. }
        | C::SetNodeFlip { .. }
        | C::SetEllipseArc { .. }
        | C::AddNodeEffect { .. }
        | C::RemoveNodeEffect { .. }
        | C::SetNodeStrokeSideWidth { .. }
        | C::InsertNode { .. }
        | C::DeleteNode { .. }
        | C::CopyNode { .. } => true,
        C::PatchNodeData { patch_json, .. } => {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(patch_json) else {
                return false;
            };
            value.as_object().is_some_and(|object| {
                object.keys().all(|key| {
                    matches!(
                        key.as_str(),
                        "x" | "y"
                            | "width"
                            | "height"
                            | "rotation"
                            | "name"
                            | "opacity"
                            | "visible"
                            | "locked"
                            | "flipX"
                            | "flipY"
                            | "fill"
                            | "stroke"
                            | "effects"
                            | "cornerRadius"
                            | "d"
                            | "anchors"
                            | "closed"
                            | "fillRule"
                            | "fontFamily"
                            | "fontSize"
                            | "fontWeight"
                            | "fontStyle"
                            | "fontFeatures"
                            | "fontVariations"
                            | "content"
                            | "lineHeight"
                            | "letterSpacing"
                            | "textAlign"
                            | "textAlignVertical"
                            | "textDecoration"
                            | "textDecorationSkipInk"
                    )
                })
            })
        }
        _ => false,
    }
}

/// Called after the existing boundary, file router and agent admission. A None
/// result is safe isolated fallback: this request has not applied any command.
pub(super) fn dispatch(
    ctx: &ConnCtx<'_>,
    line: &str,
    profile: McpAccessProfile,
    path: Option<&Path>,
    document: &Path,
    admission: &AgentAdmission<'_>,
    writes: bool,
) -> Option<Result<String, WebCanvasError>> {
    if !matches!(ctx.mode, ServeMode::Managed) {
        return None;
    }
    let groups = &ctx.authority.managed_groups;
    if !writes || !candidate(line, profile) {
        return None;
    }
    let path = path?;
    let route = ctx.authority.bound_route()?;
    if route.path.as_deref() != Some(path)
        || (route.document != document && route.path.as_deref() != Some(document))
    {
        return None;
    }
    let fence = ctx.authority.recovery.clone();
    let recovery_epoch = route.epoch;
    let generation = route.generation;
    let permit = admission.execution_permit(&ctx.authority.agent_contexts, document)?;
    let input = Request {
        line: line.to_owned(),
        profile,
        path: path.to_owned(),
        document: document.to_owned(),
        state_identity: ctx.state as *const _ as usize,
        recovery_epoch,
        generation,
        permit,
    };
    let charge = input.line.capacity()
        + input.path.as_os_str().len()
        + input.document.as_os_str().len() * 2
        + MAX_REPLY
        + 2048;
    let ticket = match groups.queue.submit(input, line.len(), charge) {
        Ok(ticket) => ticket,
        Err(_) => {
            groups
                .metrics
                .capacity_fallback
                .fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };
    groups.metrics.admitted.fetch_add(1, Ordering::Relaxed);
    match ticket.execute(
        |requests| run_group(ctx, groups, requests),
        |_| {
            fence.trip_at(recovery_epoch);
            // Closing without a tools/call response retains an unconfirmed
            // outcome. A failure after rename must never look like rejection.
            Reply::Complete(Err(WebCanvasError::Config(
                GroupError::LeaderPanicked.to_string(),
            )))
        },
    ) {
        Reply::Complete(response) => Some(response),
        Reply::Isolated if fence.required() => Some(Err(WebCanvasError::Config(
            GroupError::Unavailable.to_string(),
        ))),
        Reply::Isolated => None,
    }
}

/// The bound cache is epoch-checked independently from the writer mutex.
/// Alternate paths, stale bindings and unusual barriers keep ordinary routing.
pub(super) fn dispatch_early(
    ctx: &ConnCtx<'_>,
    line: &str,
    profile: McpAccessProfile,
    admission: &AgentAdmission<'_>,
) -> Option<Result<String, WebCanvasError>> {
    if !matches!(ctx.mode, ServeMode::Managed)
        || ctx.write_barrier.is_some()
        || !candidate(line, profile)
    {
        return None;
    }
    let route = ctx.authority.bound_route()?;
    let path = route.path.as_ref()?;
    let call = op_mcp::parse_tool_call(line)?;
    if let Some(raw) = call
        .arguments
        .get("filePath")
        .filter(|raw| !raw.trim().is_empty())
    {
        if !matches!(raw.as_str(), "live://canvas" | "live://canvas/current")
            && Path::new(raw) != path
            && Path::new(raw) != route.document
        {
            return None;
        }
    }
    dispatch(
        ctx,
        line,
        profile,
        Some(path),
        &route.document,
        admission,
        true,
    )
}

struct Pending {
    request: usize,
    capture: CommandEditCapture,
}

struct Transaction<'s, 'a, 'q> {
    state: &'s mut WebCanvasState,
    ctx: &'s ConnCtx<'a>,
    owner: &'s Groups,
    requests: &'q [Request],
    pending: Vec<Pending>,
    replies: Vec<Option<Reply>>,
    ticks: Vec<super::SseTick>,
    budget: budget::Budget,
    before_version: u64,
    last_changed: Option<usize>,
    live_selection: Option<op_editor_core::SelectionState>,
    persisted: bool,
    finished: bool,
}

impl Transaction<'_, '_, '_> {
    fn stage(&mut self, index: usize) {
        let request = &self.requests[index];
        if request.state_identity != self.ctx.state as *const _ as usize {
            self.replies[index] = Some(Reply::Isolated);
            return;
        }
        if self.state.current_path.as_ref() != Some(&request.path)
            || self.state.recovery.epoch() != request.recovery_epoch
            || self.state.editor.document_generation() != request.generation
        {
            self.replies[index] = Some(Reply::Complete(Ok(error(
                &request.line,
                &GroupError::AuthorityChanged,
            ))));
            return;
        }
        let structural = op_mcp::parse_tool_call(&request.line).is_some_and(|call| {
            matches!(
                call.tool.as_str(),
                "insert_node" | "delete_node" | "copy_node"
            )
        });
        if structural && self.live_selection.is_none() {
            let selection = &self.state.editor.selection;
            if selection.set.len() > 16_384 {
                self.replies[index] = Some(Reply::Isolated);
                return;
            }
            let bytes = selection
                .set
                .iter()
                .map(|id| id.as_str().len())
                .sum::<usize>()
                + selection.set.len() * std::mem::size_of::<op_editor_core::NodeId>()
                + selection.anchor.as_str().len()
                + std::mem::size_of::<op_editor_core::SelectionState>();
            if bytes > 2 * 1024 * 1024 || self.budget.reserve_live_selection(bytes).is_err() {
                self.replies[index] = Some(Reply::Isolated);
                return;
            }
            self.live_selection = Some(selection.clone());
        }
        let policy = self.state.collab_policy();
        let mut capture = None;
        let mut applied = false;
        let mut changed = false;
        let mut isolated = false;
        let mut refused = None;
        let mut failure = None;
        let budget = &mut self.budget;
        let before_budget = *budget;
        let fence = self.state.recovery.clone();
        let metrics = &self.owner.metrics;
        let response = mcp_serve::agent_context::with_agent_context_permit(
            &self.ctx.authority.agent_contexts,
            &request.document,
            &mut self.state.editor,
            &request.line,
            request.profile,
            &request.permit,
            |editor| {
                let operation = fence.watch();
                let result = mcp_serve::process_message_with_applier_profiled(
                    editor,
                    &request.line,
                    request.profile,
                    |_, editor, command| {
                        if capture.is_some() {
                            failure = Some(GroupError::CommandCount);
                            return false;
                        }
                        if let Err(reason) =
                            policy.check_command(command, op_editor_core::CollabEditSource::Mcp)
                        {
                            refused = Some(reason);
                            return false;
                        }
                        if !audited(command) {
                            isolated = true;
                            return false;
                        }
                        let reservation = budget.reserve(editor, command, request.line.len());
                        metrics.observe_forecast(budget.forecast());
                        if let Err(reason) = reservation {
                            metrics.refuse(reason);
                            isolated = true;
                            return false;
                        }
                        // Strict16 API: None allocates no page/full fallback.
                        let Some(checkpoint) =
                            editor.try_begin_command_edit_without_document_repairs(command)
                        else {
                            metrics
                                .strict_capture_fallback
                                .fetch_add(1, Ordering::Relaxed);
                            isolated = true;
                            return false;
                        };
                        debug_assert!(checkpoint.is_target_only());
                        capture = Some(checkpoint);
                        let before = editor.document_revision();
                        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            editor.apply_command_edit_then(
                                command.clone(),
                                capture.as_mut().unwrap(),
                                |_| {},
                            )
                        })) {
                            Ok(ok) => {
                                applied = ok;
                                changed = editor.document_revision() != before;
                                if !ok && changed {
                                    failure = Some(GroupError::PartialRefusal);
                                }
                                ok
                            }
                            Err(_) => {
                                failure = Some(GroupError::CommandPanicked);
                                false
                            }
                        }
                    },
                );
                let oversized = result
                    .as_ref()
                    .ok()
                    .and_then(Option::as_ref)
                    .is_some_and(|response| response.len() > MAX_REPLY);
                isolated |= oversized;
                if !applied || isolated || failure.is_some() || result.is_err() {
                    if let Some(checkpoint) = capture.take() {
                        if let Err(reason) = editor.abort_command_edit(checkpoint) {
                            fence.trip();
                            failure = Some(GroupError::Rollback(reason));
                        }
                    }
                    applied = false;
                    changed = false;
                    *budget = before_budget;
                }
                if applied && !changed {
                    budget.retain_unanchored_components();
                }
                operation.complete();
                result
            },
        );
        if let Some(reason) = refused {
            self.state.editor.editor_ui.collab.set_notice(
                reason.notice_kind(),
                crate::design_agent_tools::reveal_now_millis(),
            );
            self.state.collab.bump_seq();
            self.ticks.push(self.state.sse_tick());
        }
        self.replies[index] = Some(if let Some(failure) = failure {
            Reply::Complete(Ok(error(&request.line, &failure)))
        } else if isolated {
            metrics.isolated.fetch_add(1, Ordering::Relaxed);
            Reply::Isolated
        } else {
            Reply::Complete(
                response
                    .map(|body| body.unwrap_or_default())
                    .map_err(Into::into),
            )
        });
        if applied {
            metrics.applied.fetch_add(1, Ordering::Relaxed);
            self.pending.push(Pending {
                request: index,
                capture: capture.expect("applied capture"),
            });
        }
        if changed {
            self.state.version += 1;
            self.last_changed = Some(index);
            self.ticks.push(self.state.sse_tick());
        }
    }

    fn rollback(&mut self) -> bool {
        let mut clean = true;
        for pending in std::mem::take(&mut self.pending).into_iter().rev() {
            let request = &self.requests[pending.request];
            let mut capture = Some(pending.capture);
            let mut restored = false;
            #[cfg(test)]
            let fail_rollback = self.owner.fault.load(Ordering::Acquire) == 3;
            let _ = mcp_serve::agent_context::with_agent_context_permit(
                &self.ctx.authority.agent_contexts,
                &request.document,
                &mut self.state.editor,
                &request.line,
                request.profile,
                &request.permit,
                |editor| {
                    #[cfg(test)]
                    if fail_rollback {
                        editor.replace_document(editor.doc.clone());
                    }
                    restored = editor.abort_command_edit(capture.take().unwrap()).is_ok();
                    Ok(None)
                },
            );
            clean &= restored;
        }
        self.state.version = self.before_version;
        if clean {
            if let Some(selection) = self.live_selection.take() {
                self.state.editor.selection = selection;
            }
        }
        if !clean {
            self.state.recovery.trip();
        }
        clean
    }

    fn finish(mut self) -> Vec<Reply> {
        if self.state.recovery.required() {
            self.rollback();
            self.finished = true;
            return (0..self.requests.len())
                .map(|_| {
                    Reply::Complete(Err(WebCanvasError::Config(
                        GroupError::Unavailable.to_string(),
                    )))
                })
                .collect();
        }
        if let Some(index) = self.last_changed {
            let actor = &self.requests[index];
            let mut source = None;
            let mut saved = false;
            let mut started = false;
            let changed_count = (self.state.version - self.before_version) as usize;
            let persisted = &mut self.persisted;
            let fence = self.state.recovery.clone();
            let metrics = &self.owner.metrics;
            #[cfg(test)]
            let owner = self.owner;
            let _ = mcp_serve::agent_context::with_agent_context_permit(
                &self.ctx.authority.agent_contexts,
                &actor.document,
                &mut self.state.editor,
                &actor.line,
                actor.profile,
                &actor.permit,
                |editor| {
                    started = true;
                    let operation = fence.watch();
                    #[cfg(test)]
                    if owner.fault.load(Ordering::Acquire) == 1 {
                        panic!("injected before save");
                    }
                    let saved_result = crate::doc_io::save_to_path_compact(editor, &actor.path);
                    #[cfg(test)]
                    if saved_result.is_ok() && owner.fault.load(Ordering::Acquire) == 2 {
                        panic!("injected after rename before save acknowledgement");
                    }
                    match saved_result {
                        Ok(()) => {
                            saved = true;
                            *persisted = true;
                            metrics.saves.fetch_add(1, Ordering::Relaxed);
                            metrics
                                .committed_changes
                                .fetch_add(changed_count as u64, Ordering::Relaxed);
                            metrics.saved_sizes[changed_count - 1].fetch_add(1, Ordering::Relaxed);
                            #[cfg(test)]
                            owner.saves.fetch_add(1, Ordering::AcqRel);
                        }
                        Err(error) => source = Some(error),
                    }
                    operation.complete();
                    Ok(None)
                },
            );
            if !saved {
                if started && source.is_none() {
                    // The save action unwound. It may have renamed already:
                    // do not guess which canonical state is on disk or replay.
                    self.state.recovery.trip();
                    self.persisted = true;
                    self.finished = true;
                    return (0..self.requests.len())
                        .map(|_| {
                            Reply::Complete(Err(WebCanvasError::Config(
                                GroupError::LeaderPanicked.to_string(),
                            )))
                        })
                        .collect();
                }
                let accepted: Vec<_> = self.pending.iter().map(|pending| pending.request).collect();
                let clean = self.rollback();
                let failure = if !clean {
                    GroupError::RollbackContext
                } else {
                    source
                        .map(GroupError::Save)
                        .unwrap_or(GroupError::ContextDidNotSave)
                };
                for index in accepted {
                    self.replies[index] = Some(if clean {
                        Reply::Complete(Ok(error(&self.requests[index].line, &failure)))
                    } else {
                        Reply::Complete(Err(WebCanvasError::Config(
                            GroupError::Unavailable.to_string(),
                        )))
                    });
                }
                self.ctx.hub.broadcast(self.state.sse_tick());
                self.finished = true;
                return self
                    .replies
                    .iter_mut()
                    .map(|reply| reply.take().unwrap())
                    .collect();
            }
        }
        self.finished = true;
        self.pending.clear();
        for tick in &self.ticks {
            self.ctx.hub.broadcast(*tick);
        }
        self.replies
            .iter_mut()
            .map(|reply| reply.take().unwrap())
            .collect()
    }
}

impl Drop for Transaction<'_, '_, '_> {
    fn drop(&mut self) {
        if !self.finished {
            self.state.recovery.trip();
            if !self.persisted {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.rollback()));
            }
        }
    }
}

fn run_group(ctx: &ConnCtx<'_>, owner: &Groups, requests: &[Request]) -> Vec<Reply> {
    owner.metrics.cohorts.fetch_add(1, Ordering::Relaxed);
    if let Some(index) = requests.len().checked_sub(1) {
        owner.metrics.claimed_sizes[index].fetch_add(1, Ordering::Relaxed);
    }
    let mut state = ctx.state.lock().unwrap_or_else(|p| p.into_inner());
    let before_version = state.version;
    let mut transaction = Transaction {
        state: &mut state,
        ctx,
        owner,
        requests,
        pending: Vec::with_capacity(requests.len()),
        replies: (0..requests.len()).map(|_| None).collect(),
        ticks: Vec::with_capacity(requests.len()),
        budget: budget::Budget::default(),
        before_version,
        last_changed: None,
        live_selection: None,
        persisted: false,
        finished: false,
    };
    for index in 0..requests.len() {
        if transaction.state.recovery.required() {
            transaction.replies[index] = Some(Reply::Complete(Ok(error(
                &requests[index].line,
                &GroupError::Unavailable,
            ))));
        } else {
            transaction.stage(index);
        }
    }
    transaction.finish()
}

#[cfg(test)]
#[path = "managed_group_commit_tests.rs"]
mod tests;
