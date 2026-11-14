//! One locked file, many exact actor transactions, one final atomic save.
use super::{audited_command, budget, quota, Groups, Reply, ReplyValue, Request, MAX_REPLY};
use crate::{
    document_recovery::DocumentRecovery,
    mcp_serve::{self, resident_documents::FileStamp, McpServeError},
};
use op_editor_core::{CommandEditCapture, EditorState};
use std::{path::Path, sync::atomic::Ordering};

#[path = "resident_group_apply.rs"]
mod apply;
struct Pending {
    request: usize,
    capture: CommandEditCapture,
}
pub(super) struct Transaction<'s, 'q> {
    claim_epoch: u64,
    editor: &'s mut EditorState,
    path: &'s Path,
    stamp: &'s FileStamp,
    fence: &'s DocumentRecovery,
    groups: &'s Groups,
    requests: &'q [Request],
    id_sequence: op_editor_core::CommandIdSequence,
    budget: budget::Budget,
    capture_quota: quota::Reservation,
    pending: Vec<Pending>,
    replies: Vec<Option<Reply>>,
    last_changed: Option<usize>,
    live_selection: Option<op_editor_core::SelectionState>,
    changed: usize,
    fatal: Option<String>,
    persisted: bool,
    finished: bool,
}
impl<'s, 'q> Transaction<'s, 'q> {
    pub(super) fn new(
        editor: &'s mut EditorState,
        path: &'s Path,
        stamp: &'s FileStamp,
        fence: &'s DocumentRecovery,
        groups: &'s Groups,
        requests: &'q [Request],
        claim_epoch: u64,
    ) -> Self {
        Self {
            claim_epoch,
            editor,
            path,
            stamp,
            fence,
            groups,
            requests,
            id_sequence: Default::default(),
            budget: Default::default(),
            capture_quota: quota::Reservation::captures(),
            pending: Vec::with_capacity(requests.len()),
            replies: (0..requests.len()).map(|_| None).collect(),
            last_changed: None,
            live_selection: None,
            changed: 0,
            fatal: None,
            persisted: false,
            finished: false,
        }
    }
    pub(super) fn stage(&mut self, index: usize) {
        let request = &self.requests[index];
        if request.claim_epoch != self.claim_epoch {
            self.replies[index] = Some(request.error(
                "canonical file ownership changed while this request waited; no command applied",
            ));
            return;
        }
        if self.fence.required() {
            self.replies[index] = Some(request.reply(ReplyValue::Complete(Err(
                McpServeError::Io(crate::document_recovery::REQUIRED.into()),
            ))));
            return;
        }
        if let Some(reason) = &self.fatal {
            self.replies[index] = Some(request.error(reason));
            return;
        }
        if !request.route.valid() {
            let reason =
                "bound primary changed during resident cohort; no pending change acknowledged"
                    .to_owned();
            self.replies[index] = Some(request.error(&reason));
            self.fatal = Some(reason);
            return;
        }
        if let Err(error) = self.stamp.verify(self.path) {
            let reason = error.to_string();
            self.replies[index] = Some(request.error(&reason));
            self.fatal = Some(reason);
            return;
        }
        if self.live_selection.is_none()
            && op_mcp::parse_tool_call(&request.line)
                .is_some_and(|call| mcp_serve::cohort_coverage::structural_tool(&call.tool))
        {
            let before_selection_budget = self.budget;
            let selection = &self.editor.selection;
            let bytes = selection
                .set
                .len()
                .saturating_mul(std::mem::size_of::<op_editor_core::NodeId>())
                .saturating_add(
                    selection
                        .set
                        .iter()
                        .map(|id| id.as_str().len())
                        .sum::<usize>(),
                )
                .saturating_add(selection.anchor.as_str().len())
                .saturating_add(std::mem::size_of::<op_editor_core::SelectionState>());
            if selection.set.len() > 16_384
                || bytes > 2 * 1024 * 1024
                || self.budget.reserve_live_selection(bytes).is_err()
                || !self.capture_quota.grow_to(self.budget.used())
            {
                self.budget = before_selection_budget;
                self.replies[index] = Some(request.reply(ReplyValue::Isolated));
                return;
            }
            self.live_selection = Some(selection.clone());
        }
        let before_budget = self.budget;
        let budget = &mut self.budget;
        let capture_quota = &mut self.capture_quota;
        let metrics = &self.groups.metrics;
        let fence = self.fence.clone();
        let mut capture = None;
        let mut applied = false;
        let mut changed = false;
        let mut isolated = false;
        let mut failure = None;
        let mut command_count = 0;
        let response = self.id_sequence.with_authority(self.editor, |editor| {
            mcp_serve::agent_context::with_agent_context_permit(
                &request.contexts,
                self.path,
                editor,
                &request.line,
                request.profile,
                &request.permit,
                |editor| {
                    let operation = fence.watch();
                    let result = mcp_serve::process_message_with_bound_authority_profiled(
                        editor,
                        &request.line,
                        request.profile,
                        true,
                        |tool, editor, command| {
                            command_count += 1;
                            if command_count != 1 {
                                failure = Some(
                                    "resident grouped setter emitted more than one command"
                                        .to_owned(),
                                );
                                return false;
                            }
                            if !audited_command(command) {
                                isolated = true;
                                return false;
                            }
                            let before = editor.document_revision();
                            let attempt =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    apply::apply(
                                        editor,
                                        command,
                                        tool,
                                        request.line.len(),
                                        budget,
                                        capture_quota,
                                        metrics,
                                        &mut capture,
                                    )
                                }));
                            match attempt {
                                Ok(apply::Outcome::Applied(ok)) => {
                                    applied = ok;
                                    changed = editor.document_revision() != before;
                                    if !ok && changed {
                                        failure = Some(
                                            "resident grouped command refused after mutation"
                                                .to_owned(),
                                        );
                                    }
                                    ok
                                }
                                Ok(apply::Outcome::Isolated) => {
                                    isolated = true;
                                    false
                                }
                                Err(_) => {
                                    editor.invalidate_command_id_sequence();
                                    failure = Some("resident grouped command panicked".to_owned());
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
                    // No deferred external effects are admitted here. A rejected
                    // or unrepresentable bounded call can fall back only after
                    // its exact capture has demonstrably restored all local state.
                    if !applied || isolated || failure.is_some() || result.is_err() {
                        if let Some(checkpoint) = capture.take() {
                            if editor.abort_command_edit(checkpoint).is_err() {
                                fence.trip();
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
            )
        });
        let rejected = response.as_ref().is_ok_and(|body| {
            body.as_ref().is_some_and(|wire| {
                serde_json::from_str::<serde_json::Value>(wire).is_ok_and(|value| {
                    value["result"]["isError"] == true
                        || value.get("error").is_some_and(|error| !error.is_null())
                })
            })
        });
        if applied && (response.is_err() || rejected) {
            self.id_sequence.invalidate();
            // A context restoration error must not leave an unacknowledged
            // staged command in the cohort. Reenter exactly that pinned actor.
            if let Some(checkpoint) = capture.take() {
                if !rollback_one(self.editor, self.path, request, checkpoint) {
                    self.fence.trip();
                }
            }
            applied = false;
            changed = false;
            self.budget = before_budget;
        }
        self.replies[index] = Some(if let Some(reason) = failure {
            request.error(&reason)
        } else if isolated {
            metrics.isolated.fetch_add(1, Ordering::Relaxed);
            request.reply(ReplyValue::Isolated)
        } else {
            request.reply(ReplyValue::Complete(
                response.map(|body| body.unwrap_or_default()),
            ))
        });
        if applied {
            self.pending.push(Pending {
                request: index,
                capture: capture.expect("applied bounded capture"),
            });
        }
        if changed {
            self.changed += 1;
            self.last_changed = Some(index);
        }
    }

    fn rollback(&mut self) -> bool {
        self.id_sequence.invalidate();
        let mut clean = true;
        for pending in std::mem::take(&mut self.pending).into_iter().rev() {
            clean &= rollback_one(
                self.editor,
                self.path,
                &self.requests[pending.request],
                pending.capture,
            );
            self.groups
                .metrics
                .rolled_back
                .fetch_add(1, Ordering::Relaxed);
        }
        if clean {
            if let Some(selection) = self.live_selection.take() {
                self.editor.selection = selection;
            }
        } else {
            self.fence.trip();
        }
        clean
    }
    fn reject_pending(&mut self, reason: &str) {
        for pending in &self.pending {
            self.replies[pending.request] = Some(self.requests[pending.request].error(reason));
        }
    }
    pub(super) fn finish(mut self) -> (Vec<Reply>, bool, bool) {
        let mut invalidate = false;
        if self
            .pending
            .iter()
            .any(|pending| !self.requests[pending.request].route.valid())
        {
            self.fatal = Some(
                "bound primary changed before resident cohort save; pending changes rolled back"
                    .into(),
            );
        }
        if let Some(reason) = self.fatal.take() {
            self.reject_pending(&reason);
            self.rollback();
            invalidate = true;
        } else if !self.fence.required() {
            if let Some(index) = self.last_changed {
                let request = &self.requests[index];
                let mut saved = false;
                let mut save_error = None;
                let path = self.path;
                let stamp = self.stamp;
                let fence = self.fence.clone();
                let persisted = &mut self.persisted;
                let result = mcp_serve::agent_context::with_agent_context_permit(
                    &request.contexts,
                    path,
                    self.editor,
                    &request.line,
                    request.profile,
                    &request.permit,
                    |editor| {
                        let operation = fence.watch();
                        match stamp
                            .verify(path)
                            .map_err(|error| McpServeError::Document(error.to_string()))
                            .and_then(|_| mcp_serve::save_editor_state(editor, path))
                        {
                            Ok(()) => {
                                *persisted = true;
                                saved = true;
                            }
                            Err(error) => save_error = Some(error.to_string()),
                        }
                        operation.complete();
                        Ok(None)
                    },
                );
                if !saved {
                    let reason = save_error.unwrap_or_else(|| {
                        result
                            .err()
                            .map(|e| e.to_string())
                            .unwrap_or_else(|| "resident final writer context did not save".into())
                    });
                    self.reject_pending(&reason);
                    self.rollback();
                    invalidate = true;
                } else {
                    // Rename is the irreversible boundary. Context/fence
                    // failure after it is unconfirmed, never a safe rejection.
                    if result.is_err() {
                        self.fence.trip();
                    }
                    self.groups.metrics.saves.fetch_add(1, Ordering::Relaxed);
                    self.groups
                        .metrics
                        .changed
                        .fetch_add(self.changed as u64, Ordering::Relaxed);
                    self.groups.metrics.saved[self.changed - 1].fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        if self.fence.required() {
            if !self.persisted {
                self.rollback();
            }
            self.finished = true;
            return (
                self.requests
                    .iter()
                    .map(|request| {
                        request.reply(ReplyValue::Complete(Err(McpServeError::Io(
                            crate::document_recovery::REQUIRED.into(),
                        ))))
                    })
                    .collect(),
                self.persisted,
                true,
            );
        }
        self.finished = true;
        self.pending.clear();
        (
            self.replies
                .iter_mut()
                .map(|reply| reply.take().expect("one reply per resident request"))
                .collect(),
            self.persisted,
            invalidate,
        )
    }
}
impl Drop for Transaction<'_, '_> {
    fn drop(&mut self) {
        if !self.finished {
            self.fence.trip();
            if !self.persisted {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.rollback()));
            }
        }
    }
}
fn rollback_one(
    editor: &mut EditorState,
    path: &Path,
    request: &Request,
    capture: CommandEditCapture,
) -> bool {
    let mut capture = Some(capture);
    let mut restored = false;
    let _ = mcp_serve::agent_context::with_agent_context_permit(
        &request.contexts,
        path,
        editor,
        &request.line,
        request.profile,
        &request.permit,
        |editor| {
            restored = editor.abort_command_edit(capture.take().unwrap()).is_ok();
            Ok(None)
        },
    );
    restored
}
