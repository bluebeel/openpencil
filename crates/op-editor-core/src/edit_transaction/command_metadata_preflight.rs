//! Applicability proof for a closed metadata program without simulating a Doc.
use super::target_plan;
use crate::command_node_attrs::bulk::{self, Prepared};
use crate::{EditorCommand as C, EditorState};
use jian_ops_schema::node::PenNode;
use std::collections::HashMap;

// These limit only this optional optimization. A false result leaves the
// caller's complete ordinary simulation/apply fallback available unchanged.
const MAX_COMMANDS: usize = 4096;
const MAX_DEPTH: usize = 32;
const MAX_HEADER_BYTES: usize = 16 * 1024 * 1024;
const MAX_NUMERIC_PATCH_BYTES: usize = 64 * 1024;

impl EditorState {
    /// Prove that every supported metadata setter succeeds in input order at
    /// this immutable borrow, using the same exact leaf writers as Batch apply.
    /// One complete shallow metadata clone is staged per resolved target; no
    /// canonical children, document, history, actor state or catalog is cloned.
    ///
    /// True does not apply commands, grant a future revision lease or prove a
    /// durable save. The caller must retain its ordinary transaction/apply and
    /// save-before-ack boundary. Any intervening mutation invalidates the proof.
    /// Public/raw states without explicit host authority return false, as do
    /// unsupported/unprovable inputs and optional scratch-budget exhaustion.
    pub fn preflight_metadata_commands(&self, commands: &[C]) -> bool {
        let seed = &self.command_id_seed;
        if commands.is_empty()
            || !seed.entered
            || !seed.valid
            || seed.generation != self.document_generation()
            || self.editor_ui.workspace.active
            || self.instance_write_virtual_anchor.is_some()
            || self.undo_capture_suspended
        {
            return false;
        }
        preflight(self, commands).is_some()
    }
}

fn preflight(state: &EditorState, commands: &[C]) -> Option<()> {
    if commands.len() > MAX_COMMANDS {
        return None;
    }
    // Pointer keys identify actual borrowed canonical nodes, not IDs. Duplicate
    // IDs on different pages/routes cannot share a staged header accidentally.
    // No pointer survives this immutable request; none is ever dereferenced.
    let mut headers: HashMap<*const PenNode, PenNode> = HashMap::new();
    let mut pending = vec![(commands.iter(), 0usize)];
    let mut count = 0usize;
    let mut remaining = MAX_HEADER_BYTES.checked_sub(64 * 1024)?;
    while let Some((siblings, depth)) = pending.last_mut() {
        let Some(command) = siblings.next() else {
            pending.pop();
            continue;
        };
        count = count.checked_add(1)?;
        if count > MAX_COMMANDS {
            return None;
        }
        if let C::Batch { commands } = command {
            if commands.is_empty() || commands.len() > MAX_COMMANDS - count || *depth >= MAX_DEPTH {
                return None;
            }
            let next_depth = *depth + 1;
            pending.push((commands.iter(), next_depth));
            continue;
        }
        // Bound incoming clone/parse growth before preparing a rich patch or
        // allowing the existing writer to own its text/style values.
        remaining = remaining.checked_sub(incoming_bytes(command)?)?;
        let prepared = bulk::prepare(command)?;
        let specs = target_plan::plan_prepared(
            state,
            command,
            matches!(&prepared, Prepared::NumericPatch(_)),
        )?;
        let target = target_plan::metadata_target(state, &specs)?;
        let key = target as *const PenNode;
        if !headers.contains_key(&key) {
            if !state.metadata_target_preserves_components(target) {
                return None;
            }
            let fixed = std::mem::size_of::<PenNode>()
                .checked_mul(4)?
                .checked_add(128)?;
            remaining = remaining.checked_sub(fixed)?;
            let bytes =
                crate::history_snapshot::command_capture_node_bytes(target, true, remaining)
                    .ok()?;
            remaining = remaining.checked_sub(bytes)?;
            headers.insert(key, crate::pen_node_ext::clone_without_children(target));
        }
        // Success on a newly staged header is not enough: later commands on
        // that same node must see preceding effect/visibility/style changes.
        if !bulk::write(headers.get_mut(&key)?, command, &prepared) {
            return None;
        }
    }
    Some(())
}

fn incoming_bytes(command: &C) -> Option<usize> {
    let bytes = match command {
        C::UpdateNode { name, fill_hex, .. } => name
            .as_ref()
            .map_or(0, String::len)
            .checked_add(fill_hex.as_ref().map_or(0, String::len))?,
        C::PatchNodeData { patch_json, .. } if patch_json.len() <= MAX_NUMERIC_PATCH_BYTES => {
            // Closed numeric-key parsing cannot retain an arbitrary source
            // forest. Reserve its parser/Value expansion before doing it.
            return patch_json.len().checked_mul(16)?.checked_add(256);
        }
        C::SetNodeText { text, .. } => text.len(),
        C::SetNodeStrokeHex { hex, .. }
        | C::SetNodeFillHex { hex, .. }
        | C::SetEffectColor { hex, .. } => hex.len(),
        C::SetNodeName { name, .. } => name.len(),
        C::SetNodeLayoutProp {
            property, value, ..
        } => property.len().checked_add(match value {
            crate::command::LayoutPropValue::Keyword(value) => value.len(),
            crate::command::LayoutPropValue::Number(_) => 0,
            _ => return None,
        })?,
        C::SetNodeRotation { .. }
        | C::SetNodeCornerRadius { .. }
        | C::SetNodeFontSize { .. }
        | C::SetNodeFontWeight { .. }
        | C::SetNodeStrokeWidth { .. }
        | C::SetNodeStrokeSideWidth { .. }
        | C::SetNodeFlag { .. }
        | C::SetNodeFlip { .. }
        | C::AddNodeEffect { .. }
        | C::RemoveNodeEffect { .. }
        | C::SetEffectParam { .. } => 0,
        _ => return None,
    };
    bytes.checked_mul(4)?.checked_add(256)
}
