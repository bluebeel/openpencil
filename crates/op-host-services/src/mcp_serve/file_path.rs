//! Per-call `filePath` routing for the desktop MCP transports.
//!
//! The TypeScript MCP treats `filePath` as the target document for
//! most tools. The Rust desktop transports are still started with a
//! primary document path, but when a tool call supplies a different
//! `filePath`, dispatch against that file for this call only.

use op_editor_core::EditorCommand;
use std::path::{Path, PathBuf};

use super::agent_context::{default_agent_contexts, with_agent_context, AgentContexts};
use super::McpServeError;
#[path = "file_path_resolution.rs"]
mod resolution;
use resolution::RequestPaths;

const LIVE_CANVAS_SENTINELS: &[&str] = &["live://canvas", "live://canvas/current"];
static COPIES: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct TargetFileCall {
    tool: String,
    path: PathBuf,
}

/// Primary owners provide a borrowed save callback instead of cloning or
/// reloading their live editor. Coordination is acquired before that callback
/// takes the primary lock; destination authority is held through atomic save.
pub(crate) fn process_live_save_for_file_path_arg(
    current_path: Option<&Path>,
    line: &str,
    profile: super::tool_profile::McpAccessProfile,
    save: impl FnOnce(&Path) -> Result<(), McpServeError>,
) -> Result<Option<String>, McpServeError> {
    use super::resident_documents::{resident_documents, ActionResult, AuthorityError};
    let Some(call) = op_mcp::parse_tool_call(line.trim()) else {
        return Ok(None);
    };
    if call.tool != "save_document" || profile.deny_unshareable_tools {
        return Ok(None);
    }
    if let Some(refusal) = profile.refuse_call(&call.tool, &call.arguments) {
        return Ok(Some(error_response(&call, refusal.message(&call.tool))));
    }
    let mut paths = RequestPaths::default();
    if let Some(source) = call.arguments.get("sourceFilePath") {
        if !LIVE_CANVAS_SENTINELS.contains(&source.as_str())
            && !current_path.is_some_and(|current| paths.same(Path::new(source), current))
        {
            return Ok(None);
        }
    }
    let Some(destination) = call
        .arguments
        .get("filePath")
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let result = (|| {
        let _coordination = COPIES.lock().map_err(|_| AuthorityError::Poisoned)?;
        paths.after_coordination_wait();
        let destination = paths.resolve(Path::new(destination))?;
        let saved = if paths.is_current(&destination, current_path) {
            save(destination.as_path())
        } else {
            resident_documents()
                .acquire_canonical(&destination)?
                .with_destination(|path| {
                    let saved = save(path);
                    ActionResult {
                        persisted: saved.is_ok(),
                        invalidate: false,
                        value: saved,
                    }
                })?
        };
        Ok::<_, AuthorityError>(save_response(&call, destination.as_path(), &saved))
    })();
    Ok(Some(match result {
        Ok(response) => response,
        Err(AuthorityError::Uncertain) => {
            return Err(McpServeError::Io(crate::document_recovery::REQUIRED.into()))
        }
        Err(error) => error_response(&call, error.to_string()),
    }))
}

pub(super) fn process_message_for_primary(
    state: &mut op_editor_core::EditorState,
    path: &Path,
    line: &str,
    profile: super::tool_profile::McpAccessProfile,
) -> Result<Option<String>, McpServeError> {
    let contexts = default_agent_contexts();
    let _agent = match contexts.try_admit(line, profile) {
        Ok(admission) => admission,
        Err(error) => {
            return Ok(op_mcp::parse_tool_call(line.trim())
                .map(|call| error_response(&call, error.to_string())))
        }
    };
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(response) =
        process_live_save_for_file_path_arg(Some(path), line, profile, |destination| {
            with_agent_context(contexts, &key, state, line, profile, |state| {
                super::save_editor_state(state, destination)?;
                Ok(None)
            })
            .map(|_| ())
        })?
    {
        return Ok(Some(response));
    }
    process_message_for_file_path_arg_with_contexts(contexts, Some(path), line, profile)
}

/// `profile` is the session's: a call retargeted at another file is served
/// under the same catalog and deployment rules as one against the primary.
pub fn process_message_for_file_path_arg(
    current_path: Option<&Path>,
    line: &str,
    profile: super::tool_profile::McpAccessProfile,
) -> Result<Option<String>, McpServeError> {
    let contexts = default_agent_contexts();
    let _agent = match contexts.try_admit(line, profile) {
        Ok(admission) => admission,
        Err(error) => {
            return Ok(op_mcp::parse_tool_call(line.trim())
                .map(|call| error_response(&call, error.to_string())))
        }
    };
    process_message_for_file_path_arg_with_contexts(contexts, current_path, line, profile)
}

pub(crate) fn process_message_for_file_path_arg_with_contexts(
    contexts: &AgentContexts,
    current_path: Option<&Path>,
    line: &str,
    profile: super::tool_profile::McpAccessProfile,
) -> Result<Option<String>, McpServeError> {
    if super::sniff::is_control_message(line) {
        return Ok(None);
    }
    let Some(call) = op_mcp::parse_tool_call(line.trim()) else {
        return Ok(None);
    };
    if let Some(refusal) = profile.refuse_call(&call.tool, &call.arguments) {
        return Ok(Some(error_response(&call, refusal.message(&call.tool))));
    }
    // Online calls retain their in-memory/account-scoped behavior. A supplied
    // path must never reach canonicalization, stat, create, or load there.
    if profile.deny_unshareable_tools {
        return Ok(None);
    }
    let mut paths = RequestPaths::default();
    // A primary-source copy must borrow its live editor, not reload a disk
    // snapshot or race an alternate-file writer through the old save tool.
    if call.tool == "save_document" {
        if let (Some(current), Some(destination)) = (current_path, call.arguments.get("filePath")) {
            let source_is_primary = call
                .arguments
                .get("sourceFilePath")
                .is_none_or(|source| paths.same(Path::new(source), current));
            if source_is_primary && !paths.same(Path::new(destination), current) {
                let conflict = super::resident_documents::AuthorityError::BoundPrimaryConflict {
                    path: current.to_path_buf(),
                };
                return Ok(Some(error_response(&call, conflict.to_string())));
            }
        }
    }
    let Some(target) = target_file_call(&call, current_path, &mut paths) else {
        return Ok(None);
    };
    let hub = super::resident_documents::resident_documents();
    let result = if target.tool == "save_document" {
        process_save_copy(
            hub,
            &target.path,
            current_path,
            &call,
            line,
            profile,
            contexts,
            &mut paths,
        )
    } else {
        paths
            .resolve(&target.path)
            .and_then(|path| hub.acquire_canonical(&path))
            .and_then(|lease| {
                lease.with_editor_accounted(
                    target.tool == "open_document",
                    |state, stamp, fence| {
                        let mut document_delta =
                            super::resident_charge::DocumentDelta::begin(state);
                        let mut persisted = false;
                        let mut invalidate = false;
                        let mut write_failure = None;
                        let response = with_agent_context(
                            contexts,
                            lease.path(),
                            state,
                            line,
                            profile,
                            |state| {
                                let operation = fence.watch();
                                let result = super::process_message_with_bound_authority_actions(
                                    state,
                                    line,
                                    profile,
                                    true,
                                    |tool, state, action| {
                                        let cmd = match action {
                                            super::AuthorityAction::Command(command) => command,
                                            super::AuthorityAction::AuthorizeAtomic(_) => {
                                                if let Err(error) = stamp.verify(lease.path()) {
                                                    write_failure = Some(error.to_string());
                                                    invalidate = true;
                                                    return false;
                                                }
                                                return true;
                                            }
                                            super::AuthorityAction::CommitAtomic(summary) => {
                                                document_delta.observe(
                                                    (
                                                        summary.previous_generation,
                                                        summary.previous_revision,
                                                    ),
                                                    state,
                                                    None,
                                                    lease.accounting_limit(),
                                                );
                                                let needs_save = summary.document_changed(state)
                                                    || (!call.arguments.contains_key("agentId")
                                                        && (summary.previous_active_page
                                                            != state.ui.active_page_index
                                                            || summary.previous_preserve_geometry
                                                                != state
                                                                    .editor_ui
                                                                    .preserve_authored_geometry));
                                                if !needs_save {
                                                    return true;
                                                }
                                                let saved = stamp
                                                    .verify(lease.path())
                                                    .map_err(|error| {
                                                        McpServeError::Document(error.to_string())
                                                    })
                                                    .and_then(|_| {
                                                        super::save_editor_state(
                                                            state,
                                                            lease.path(),
                                                        )
                                                    });
                                                if let Err(error) = saved {
                                                    write_failure = Some(error.to_string());
                                                    invalidate = true;
                                                    return false;
                                                }
                                                persisted = true;
                                                return true;
                                            }
                                            super::AuthorityAction::AtomicUncertain(reason) => {
                                                fence.trip();
                                                invalidate = true;
                                                write_failure = Some(format!(
                                                    "atomic recovery required: {reason}"
                                                ));
                                                return false;
                                            }
                                        };
                                        if let Err(error) = stamp.verify(lease.path()) {
                                            write_failure = Some(error.to_string());
                                            invalidate = true;
                                            return false;
                                        }
                                        let revision = state.document_revision();
                                        let generation = state.document_generation();
                                        let meta = (
                                            state.ui.active_page_index,
                                            state.editor_ui.preserve_authored_geometry,
                                        );
                                        let repair = super::write_repairs::generated_tool(tool);
                                        let mut capture = (cmd.may_change_document()
                                            || matches!(cmd, EditorCommand::Batch { .. }))
                                        .then(|| {
                                            super::begin_command_capture_moving_clipboard(
                                                state, cmd, repair,
                                            )
                                        });
                                        let page_ui_before = (!cmd.may_change_document()
                                            && matches!(
                                                cmd,
                                                EditorCommand::SetActivePage { .. }
                                                    | EditorCommand::Batch { .. }
                                            ))
                                        .then(|| (state.selection.clone(), state.viewport));
                                        let clipboard_before =
                                            (matches!(cmd, EditorCommand::CutSelected)
                                                && capture.as_ref().is_none_or(|capture| {
                                                    !capture.owns_replaced_clipboard_rollback()
                                                }))
                                            .then(|| std::mem::take(&mut state.clipboard));
                                        let applied = match capture.as_mut() {
                                            Some(capture) => {
                                                super::apply_command_with_command_capture(
                                                    state,
                                                    cmd.clone(),
                                                    capture,
                                                )
                                            }
                                            None => super::apply_command_with_undo(
                                                state,
                                                cmd.clone(),
                                                repair,
                                            ),
                                        };
                                        if !applied {
                                            if let Some(clipboard) = clipboard_before {
                                                state.clipboard = clipboard;
                                            }
                                            return false;
                                        }
                                        document_delta.observe(
                                            (generation, revision),
                                            state,
                                            capture.as_ref(),
                                            lease.accounting_limit(),
                                        );
                                        let needs_save = state.document_revision() != revision
                                            || (!call.arguments.contains_key("agentId")
                                                && meta
                                                    != (
                                                        state.ui.active_page_index,
                                                        state.editor_ui.preserve_authored_geometry,
                                                    ));
                                        if !needs_save {
                                            return true;
                                        }
                                        let saved = stamp
                                            .verify(lease.path())
                                            .map_err(|error| {
                                                McpServeError::Document(error.to_string())
                                            })
                                            .and_then(|_| {
                                                super::save_editor_state(state, lease.path())
                                            });
                                        if let Err(error) = saved {
                                            let rollback = if let Some(capture) = capture {
                                                capture.abort(state)
                                            } else {
                                                state.ui.active_page_index = meta.0;
                                                state.editor_ui.preserve_authored_geometry = meta.1;
                                                if let Some((selection, viewport)) = page_ui_before
                                                {
                                                    state.selection = selection;
                                                    state.viewport = viewport;
                                                }
                                                Ok(())
                                            };
                                            if let Some(clipboard) = clipboard_before {
                                                state.clipboard = clipboard;
                                            }
                                            write_failure = Some(match rollback {
                                                Ok(()) => error.to_string(),
                                                Err(rollback) => {
                                                    fence.trip();
                                                    format!("{error}; rollback failed: {rollback}")
                                                }
                                            });
                                            invalidate = true;
                                            return false;
                                        }
                                        persisted = true;
                                        true
                                    },
                                );
                                operation.complete();
                                result
                            },
                        );
                        let response = match write_failure {
                            Some(message) => Ok(Some(error_response(&call, message))),
                            None => response,
                        };
                        let document_delta = document_delta.finish(state);
                        (
                            super::resident_documents::ActionResult {
                                value: response,
                                persisted,
                                invalidate,
                            },
                            document_delta,
                        )
                    },
                )
            })
    };
    match result {
        Ok(response) => response,
        Err(super::resident_documents::AuthorityError::Uncertain) => {
            Err(McpServeError::Io(crate::document_recovery::REQUIRED.into()))
        }
        Err(error) => Ok(Some(error_response(&call, error.to_string()))),
    }
}

pub(crate) fn error_response(call: &op_mcp::ToolCall, message: String) -> String {
    op_mcp::tool_response_to_json(&op_mcp::ToolResponse::Err {
        id: call.id.clone(),
        code: op_mcp::ToolErrorCode::ToolFailed,
        message,
    })
}

fn process_save_copy(
    hub: &super::resident_documents::ResidentDocuments,
    source: &Path,
    current_path: Option<&Path>,
    call: &op_mcp::ToolCall,
    line: &str,
    profile: super::tool_profile::McpAccessProfile,
    contexts: &AgentContexts,
    paths: &mut RequestPaths,
) -> Result<Result<Option<String>, McpServeError>, super::resident_documents::AuthorityError> {
    use super::resident_documents::{ActionResult, AuthorityError};
    // Serialize copy coordination BEFORE any source/destination writer locks.
    // Ordinary per-file writes still run independently on unrelated files.
    let _copy = COPIES.lock().map_err(|_| AuthorityError::Poisoned)?;
    paths.after_coordination_wait();
    let Some(destination) = call
        .arguments
        .get("filePath")
        .filter(|path| !path.trim().is_empty())
    else {
        return Ok(Ok(Some(error_response(
            call,
            "filePath is required".into(),
        ))));
    };
    let destination = paths.resolve(Path::new(destination))?;
    let source = paths.resolve(source)?;
    if paths.is_current(&destination, current_path) && source.as_path() != destination.as_path() {
        return Err(AuthorityError::BoundPrimaryConflict {
            path: destination.as_path().to_path_buf(),
        });
    }
    let source = hub.acquire_canonical(&source)?;
    let destination_lease = (source.path() != destination.as_path())
        .then(|| hub.acquire_canonical(&destination))
        .transpose()?;
    source.with_editor_accounted(false, |state, stamp, fence| {
        // This save callback only borrows canonical content. Context exchange
        // may change runtime/history accounting, but never the live Doc charge.
        let document_delta = super::resident_charge::DocumentDelta::begin(state);
        let mut persisted = false;
        let response = with_agent_context(contexts, source.path(), state, line, profile, |state| {
            let operation = fence.watch();
            let saved = if let Some(destination_lease) = &destination_lease {
                destination_lease
                    .with_destination(|path| {
                        let result = stamp
                            .verify(source.path())
                            .map_err(|e| McpServeError::Document(e.to_string()))
                            .and_then(|_| super::save_editor_state(state, path));
                        ActionResult {
                            persisted: result.is_ok(),
                            invalidate: false,
                            value: result,
                        }
                    })
                    .map_err(|e| {
                        if matches!(e, AuthorityError::Uncertain) {
                            fence.trip();
                        }
                        McpServeError::Document(e.to_string())
                    })
                    .and_then(|result| result)
            } else {
                stamp
                    .verify(source.path())
                    .map_err(|e| McpServeError::Document(e.to_string()))
                    .and_then(|_| super::save_editor_state(state, source.path()))
            };
            persisted = destination_lease.is_none() && saved.is_ok();
            operation.complete();
            Ok(Some(save_response(call, destination.as_path(), &saved)))
        });
        let document_delta = document_delta.finish(state);
        (
            ActionResult {
                value: response,
                persisted,
                invalidate: false,
            },
            document_delta,
        )
    })
}

fn save_response(
    call: &op_mcp::ToolCall,
    destination: &Path,
    saved: &Result<(), McpServeError>,
) -> String {
    match saved {
        Ok(()) => op_mcp::tool_response_to_json(&op_mcp::ToolResponse::Ok {
            id: call.id.clone(),
            result: std::collections::BTreeMap::from([
                ("ok".into(), "true".into()),
                ("filePath".into(), destination.display().to_string()),
            ]),
            command: None,
            json: None,
            image: None,
        }),
        Err(error) => error_response(call, error.to_string()),
    }
}

fn target_file_call(
    call: &op_mcp::ToolCall,
    current_path: Option<&Path>,
    paths: &mut RequestPaths,
) -> Option<TargetFileCall> {
    let source_arg = if call.tool == "save_document" {
        "sourceFilePath"
    } else {
        "filePath"
    };
    let Some(raw) = call
        .arguments
        .get(source_arg)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return None;
    };
    if LIVE_CANVAS_SENTINELS.contains(&raw) {
        return None;
    }
    let target = PathBuf::from(raw);
    if current_path.is_some_and(|current| paths.same(&target, current)) {
        return None;
    }
    Some(TargetFileCall {
        tool: call.tool.clone(),
        path: target,
    })
}
