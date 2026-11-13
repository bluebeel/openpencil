//! File-backed stdio/HTTP dispatch and automatic finalization.

use super::*;

pub(super) fn process_message_with_auto_finalize(
    state: &mut EditorState,
    path: &Path,
    line: &str,
    mut auto_finalize: Option<&mut auto_finalize::AutoFinalize>,
    profile: tool_profile::McpAccessProfile,
) -> Result<Option<String>, McpServeError> {
    if let Some(response) = file_path::process_message_for_primary(state, path, line, profile)? {
        return Ok(Some(response));
    }
    let (explicit_finalize, annotation_only, agent_owned) = op_mcp::parse_tool_call(line.trim())
        .map(|call| {
            (
                call.tool == "finalize_design",
                call.tool == "annotate_image",
                call.arguments.contains_key("agentId"),
            )
        })
        .unwrap_or_default();
    let mut applier_failed: Option<String> = None;
    let mut apply_rejected = false;
    // File-backed mode has no live canvas for any tool call to animate —
    // the tool name is accepted (shared signature with the live-MCP path)
    // and deliberately ignored here.
    let response = agent_context::with_agent_context(
        agent_context::default_agent_contexts(),
        path,
        state,
        line,
        profile,
        |state| {
            process_message_with_applier_profiled(state, line, profile, |_, state, cmd| {
                // `EditorState::apply` runs the pre-validate-then-mutate
                // discipline; `false` means the command rejected and the
                // document was NOT changed.
                let revision_before = state.document_revision();
                let metadata_before =
                    (!agent_owned).then(|| op_pen_loader::EditorMeta::from_state(state));
                if !state.apply(cmd.clone()) {
                    apply_rejected = true;
                    return false;
                }
                if !annotation_only {
                    repair_after_document_change(state, revision_before);
                }
                let document_changed = state.document_revision() != revision_before;
                let metadata_changed = metadata_before
                    .is_some_and(|before| before != op_pen_loader::EditorMeta::from_state(state));
                if document_changed || metadata_changed {
                    if let Err(e) = save_editor_state(state, path) {
                        applier_failed = Some(format!("save failed: {e}"));
                        return false;
                    }
                }
                if !document_changed {
                    return true;
                }
                if annotation_only {
                    if let Some(auto_finalize) = auto_finalize.as_deref_mut() {
                        auto_finalize.note_scoped_annotation(state.document_revision());
                    }
                } else {
                    if let Some(auto_finalize) = auto_finalize.as_deref_mut() {
                        auto_finalize.note_write(Instant::now());
                    }
                }
                true
            })
        },
    )?;
    if let Some(ref msg) = applier_failed {
        eprintln!("openpencil-desktop mcp: {msg}");
    }
    if explicit_finalize
        && !apply_rejected
        && applier_failed.is_none()
        && response.as_deref().is_some_and(mcp_response_succeeded)
    {
        if let Some(auto_finalize) = auto_finalize {
            auto_finalize.note_finalize(state.document_revision());
        }
    }
    Ok(response)
}
