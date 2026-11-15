//! Private reply evidence for ordinary kit commands. Public MCP bool appliers
//! keep their original API; a descriptor alone never implies a created ID.
use super::AppliedReply;
use op_editor_core::{EditorCommand, EditorState, KitRootCapture};
pub(in crate::mcp_serve) fn apply(
    state: &mut EditorState,
    command: &EditorCommand,
    run: impl FnOnce(&mut EditorState) -> bool,
) -> AppliedReply {
    let capture = KitRootCapture::begin(state, command);
    let applied = run(state);
    let kit_root = capture.and_then(|capture| capture.finish(state, applied));
    AppliedReply { applied, kit_root }
}
