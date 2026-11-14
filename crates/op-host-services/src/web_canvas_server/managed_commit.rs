//! Managed primary command commits: save before acknowledgement, or roll back.
//!
//! This uses the existing flushed sibling-file/atomic-rename save contract. It
//! does not add fsync guarantees or change direct filesystem tools/REST routes.

use super::{connection::ConnCtx, ServeMode};
use crate::{doc_io::DocIoError, mcp_serve, web_canvas_server_error::WebCanvasError};
use op_editor_core::{CommandEditError, EditorCommand, EditorState};
use std::path::Path;

#[derive(Debug)]
enum CommitError {
    AuthorityChanged,
    AtomicRecovery(String),
    Save {
        source: DocIoError,
        rollback: Option<CommandEditError>,
    },
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AuthorityChanged => f.write_str(
                "managed primary changed while routing this write; no command was applied",
            ),
            Self::AtomicRecovery(reason) => write!(f, "atomic recovery required: {reason}"),
            Self::Save { source, rollback } => {
                write!(f, "managed primary save failed: {source}")?;
                if let Some(error) = rollback {
                    write!(f, "; rollback failed: {error}")?;
                } else {
                    f.write_str("; command rolled back")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for CommitError {}

fn apply_command(
    editor: &mut EditorState,
    command: &EditorCommand,
    path: Option<&Path>,
    repair: bool,
    fence: &crate::document_recovery::DocumentRecovery,
    changes: &mut super::changefeed::Staged,
) -> Result<bool, CommitError> {
    let Some(path) = path.filter(|_| {
        command.may_change_document() || matches!(command, EditorCommand::Batch { .. })
    }) else {
        let applied = mcp_serve::apply_command_with_undo(editor, command.clone(), repair);
        if applied {
            // Local/Online writes have no confirmed bound-file save boundary.
            changes.unsupported();
        }
        return Ok(applied);
    };
    let operation = fence.watch();
    let revision = editor.document_revision();
    let mut capture = mcp_serve::begin_command_capture_moving_clipboard(editor, command, repair);
    // Targeted Cut checkpoints already own this actor value. Broad repair or
    // unsupported captures keep the legacy move-aside rollback contract.
    let clipboard = (matches!(command, EditorCommand::CutSelected)
        && !capture.owns_replaced_clipboard_rollback())
    .then(|| std::mem::take(&mut editor.clipboard));
    let applied =
        mcp_serve::apply_command_with_command_capture(editor, command.clone(), &mut capture);
    if !applied {
        let rolled_back = capture.abort(editor).is_ok();
        if !rolled_back {
            fence.trip();
        }
        if let Some(clipboard) = clipboard {
            editor.clipboard = clipboard;
        }
        operation.complete();
        return Ok(false);
    }
    if editor.document_revision() != revision {
        if let Err(source) = crate::doc_io::save_to_path_compact(editor, path) {
            let rollback = capture.abort(editor).err();
            if rollback.is_some() {
                fence.trip();
            }
            if let Some(clipboard) = clipboard {
                editor.clipboard = clipboard;
            }
            operation.complete();
            return Err(CommitError::Save { source, rollback });
        }
        // The exact completed capture sees final state before any subsequent
        // command. Keep the recovery watch open during encoding as well.
        changes.push(capture.forward_metadata_json(editor, super::changefeed::MAX_RECORD_BYTES));
    }
    operation.complete();
    Ok(true)
}

fn error_response(line: &str, error: &CommitError) -> String {
    op_mcp::parse_tool_call(line)
        .map(|call| mcp_serve::file_path::error_response(&call, error.to_string()))
        .unwrap_or_default()
}

/// Dispatch under the primary authority lock. Successful command commits are
/// visible on disk before their version/SSE tick and tools/call response.
pub(super) fn dispatch(
    ctx: &ConnCtx<'_>,
    line: &str,
    profile: mcp_serve::tool_profile::McpAccessProfile,
    routed_path: Option<&Path>,
    document_key: &Path,
    agent_owned: bool,
    writes: bool,
) -> Result<String, WebCanvasError> {
    dispatch_inner(
        ctx,
        line,
        profile,
        routed_path,
        document_key,
        agent_owned,
        writes,
        None,
    )
}
pub(super) fn dispatch_pending(
    ctx: &ConnCtx<'_>,
    line: &str,
    profile: mcp_serve::tool_profile::McpAccessProfile,
    routed_path: Option<&Path>,
    document_key: &Path,
    expected: (u64, u64),
) -> Result<String, WebCanvasError> {
    dispatch_inner(
        ctx,
        line,
        profile,
        routed_path,
        document_key,
        true,
        true,
        Some(expected),
    )
}

fn dispatch_inner(
    ctx: &ConnCtx<'_>,
    line: &str,
    profile: mcp_serve::tool_profile::McpAccessProfile,
    routed_path: Option<&Path>,
    document_key: &Path,
    agent_owned: bool,
    writes: bool,
    expected: Option<(u64, u64)>,
) -> Result<String, WebCanvasError> {
    let mut state = ctx.state.lock().unwrap_or_else(|p| p.into_inner());
    if expected.is_some_and(|(epoch, generation)| {
        state.recovery.epoch() != epoch || state.editor.document_generation() != generation
    }) {
        return Ok(error_response(line, &CommitError::AuthorityChanged));
    }

    let managed = matches!(ctx.mode, ServeMode::Managed);
    state.ensure_document_available()?;
    let fence = state.recovery.clone();
    // Routing released the lock to acquire alternate-file authority. Never
    // apply an old primary request to a newly opened document/context key.
    if managed && writes && state.current_path.as_deref() != routed_path {
        return Ok(error_response(line, &CommitError::AuthorityChanged));
    }
    let commit_path = managed.then(|| state.current_path.clone()).flatten();
    let before = state.version;
    let mut committed_changes = super::changefeed::Staged::default();
    let mut applied_any = false;
    let policy = state.collab_policy();
    let mut refused = None;
    let mut failed = None;
    let atomic_watch = (managed
        && commit_path.is_some()
        && op_mcp::parse_tool_call(line).is_some_and(|call| call.tool == "batch_tools"))
    .then(|| fence.watch());
    let response = mcp_serve::agent_context::with_agent_context(
        &ctx.authority.agent_contexts,
        document_key,
        &mut state.editor,
        line,
        profile,
        |editor| {
            mcp_serve::process_message_with_bound_authority_actions(
                editor,
                line,
                profile,
                managed && commit_path.is_some(),
                |tool, editor, action| {
                    if failed.is_some()
                        && !matches!(action, mcp_serve::AuthorityAction::AtomicUncertain(_))
                    {
                        return false;
                    }
                    let command = match action {
                        mcp_serve::AuthorityAction::Command(command) => command,
                        mcp_serve::AuthorityAction::AuthorizeAtomic(command) => {
                            if let Err(reason) =
                                policy.check_command(command, op_editor_core::CollabEditSource::Mcp)
                            {
                                refused = Some(reason);
                                return false;
                            }
                            return true;
                        }
                        mcp_serve::AuthorityAction::CommitAtomic(summary) => {
                            if summary.document_changed(editor) {
                                if let Some(path) = commit_path.as_deref() {
                                    if let Err(source) =
                                        crate::doc_io::save_to_path_compact(editor, path)
                                    {
                                        failed = Some(CommitError::Save {
                                            source,
                                            rollback: None,
                                        });
                                        return false;
                                    }
                                }
                                // Mixed structure keeps the existing reset
                                // changefeed contract rather than claiming a
                                // metadata-only forward record.
                                committed_changes.unsupported();
                            }
                            applied_any |= summary.command_count != 0
                                && (!agent_owned || summary.document_changed(editor));
                            return true;
                        }
                        mcp_serve::AuthorityAction::AtomicUncertain(reason) => {
                            fence.trip();
                            failed = Some(CommitError::AtomicRecovery(reason.into()));
                            return false;
                        }
                    };
                    if let Err(reason) =
                        policy.check_command(command, op_editor_core::CollabEditSource::Mcp)
                    {
                        refused = Some(reason);
                        return false;
                    }
                    let revision = editor.document_revision();
                    match apply_command(
                        editor,
                        command,
                        commit_path.as_deref(),
                        mcp_serve::write_repairs::generated_tool(tool),
                        &fence,
                        &mut committed_changes,
                    ) {
                        Ok(applied) => {
                            applied_any |=
                                applied && (!agent_owned || editor.document_revision() != revision);
                            applied
                        }
                        Err(error) => {
                            failed = Some(error);
                            false
                        }
                    }
                },
            )
        },
    )?
    .unwrap_or_default();
    if let Some(watch) = atomic_watch {
        watch.complete();
    }
    state.ensure_document_available()?;
    if let Some(reason) = refused {
        state.editor.editor_ui.collab.set_notice(
            reason.notice_kind(),
            crate::design_agent_tools::reveal_now_millis(),
        );
        state.collab.bump_seq();
    }
    if applied_any {
        state.version += 1;
        if !managed || commit_path.is_none() {
            committed_changes.unsupported();
        }
        let version = state.version;
        state.publish_committed_changes(before, version, committed_changes);
    }
    if state.version != before || refused.is_some() {
        ctx.hub.broadcast(state.sse_tick());
    }
    Ok(match failed {
        Some(error) => error_response(line, &error),
        None => response,
    })
}

#[cfg(test)]
#[path = "managed_commit_tests.rs"]
mod tests;
