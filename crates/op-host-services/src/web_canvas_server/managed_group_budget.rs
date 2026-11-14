//! Conservative additive staging budget. Ineligible calls keep isolated dispatch.
//! This bounds extra group retention, not the complete engine's allocator RSS.
use jian_ops_schema::node::PenNode;
use op_editor_core::{
    EditorCommand as C, EditorState, HistoryEntry, NodeId, PenNodeExt, SharedNode,
};
use std::{io::Write, mem::size_of, sync::Arc};

const LIMIT: usize = 4 * 1024 * 1024;
const TARGET_JSON: usize = 16 * 1024;
const AUX_JSON: usize = 8 * 1024;
const EVICTED_BYTES: usize = 128 * 1024;

#[derive(Default, Clone, Copy)]
pub(super) struct Budget(usize);

struct Counter {
    bytes: usize,
    limit: usize,
}
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.saturating_add(bytes.len());
        if self.bytes > self.limit {
            return Err(std::io::ErrorKind::OutOfMemory.into());
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn json_bytes(value: &impl serde::Serialize, limit: usize) -> Option<usize> {
    let mut count = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut count, value).ok()?;
    Some(count.bytes)
}

fn target<'a>(state: &'a EditorState, command: &C) -> Option<&'a PenNode> {
    let (id, page) = match command {
        C::UpdateNode {
            node_id, page_id, ..
        }
        | C::PatchNodeData {
            node_id, page_id, ..
        } => (node_id, page_id.as_deref()),
        C::SetNodeRotation { node_id, .. }
        | C::SetNodeText { node_id, .. }
        | C::SetNodeCornerRadius { node_id, .. }
        | C::SetNodeFontSize { node_id, .. }
        | C::SetNodeFontWeight { node_id, .. }
        | C::SetNodeStrokeHex { node_id, .. }
        | C::SetNodeStrokeWidth { node_id, .. }
        | C::SetNodeFillHex { node_id, .. }
        | C::SetNodeName { node_id, .. } => (node_id, None),
        _ => return None,
    };
    let pages = state.doc.pages.as_ref().filter(|pages| !pages.is_empty());
    let raw = page.map(str::trim).filter(|page| !page.is_empty());
    let nodes = match pages {
        None if raw.is_none_or(|page| page == "0") => &state.doc.children,
        None => return None,
        Some(pages) => {
            let index = match raw {
                None => state.ui.active_page_index,
                Some(raw) => pages
                    .iter()
                    .position(|page| page.id == raw)
                    .or_else(|| raw.parse().ok())?,
            };
            &pages.get(index)?.children
        }
    };
    let node = op_editor_core::walkers::find_node(nodes, id)?;
    if matches!(node, PenNode::Ref(_))
        || node.children().is_some_and(|children| !children.is_empty())
    {
        return None;
    }
    Some(node)
}

fn small_evicted_entry(state: &EditorState) -> Option<usize> {
    if state.history.past.len() < op_editor_core::history::HISTORY_CAP {
        return Some(0);
    }
    let first = state.history.past.front()?.scoped_snapshot()?;
    let last = state.history.past.back()?;
    let components = match last {
        HistoryEntry::Full(snapshot) => &snapshot.components,
        HistoryEntry::Scoped(snapshot) => snapshot.components(),
    };
    // Different component owners could keep a large obsolete prototype forest
    // alive solely through a pending capture. Let the isolated writer evict it.
    if first.components().backing_identity() != components.backing_identity()
        || !Arc::ptr_eq(
            first.components().components_arc(),
            components.components_arc(),
        )
    {
        return None;
    }
    let mut bytes = first.storage_bytes();
    if bytes > EVICTED_BYTES {
        return None;
    }
    let mut pending: Vec<&Arc<SharedNode>> = first.retained_roots().take(129).collect();
    if pending.len() > 128 {
        return None;
    }
    let mut nodes = 0usize;
    while let Some(node) = pending.pop() {
        nodes += 1;
        if nodes > 128 {
            return None;
        }
        bytes = bytes.checked_add(node.storage_bytes())?;
        bytes = bytes.checked_add(
            json_bytes(node.metadata(), EVICTED_BYTES.saturating_sub(bytes))?.saturating_mul(32),
        )?;
        if let Some(children) = node.children_arc() {
            bytes = bytes.checked_add(
                children
                    .capacity()
                    .saturating_mul(size_of::<Arc<SharedNode>>() * 2),
            )?;
            if children.len() > 128usize.saturating_sub(nodes) {
                return None;
            }
            pending.extend(children.iter());
        }
        if bytes > EVICTED_BYTES {
            return None;
        }
    }
    Some(bytes)
}

impl Budget {
    pub(super) fn reserve(&mut self, state: &EditorState, command: &C, input: usize) -> bool {
        // A redo stack or a global app-state ownership map can be large even
        // when the requested leaf is small. Keep those on isolated dispatch.
        if state.editor_ui.workspace.active
            || !state.history.future.is_empty()
            || state.doc.state.is_some()
            || !state.app_state_owner.is_empty()
        {
            return false;
        }
        // Legacy component forests can be rebuilt from a stale history anchor
        // even for no-ops. Only document-backed component metadata is grouped.
        if !state.components.components.is_empty() {
            return false;
        }
        let Some(node) = target(state, command) else {
            return false;
        };
        let Some(old) = json_bytes(node, TARGET_JSON) else {
            return false;
        };
        let mut aux = state.ui.variables.active_theme.len().saturating_mul(256);
        for (key, value) in &state.ui.variables.active_theme {
            aux = aux
                .saturating_add(key.capacity())
                .saturating_add(value.capacity());
        }
        for refs in [
            &state.ui.variables.fill_refs,
            &state.ui.variables.stroke_refs,
        ] {
            aux = aux.saturating_add(
                refs.capacity()
                    .saturating_mul(size_of::<(NodeId, String)>() * 2),
            );
            if aux > AUX_JSON {
                return false;
            }
            for (key, value) in refs {
                aux = aux
                    .saturating_add(key.as_str().len())
                    .saturating_add(value.capacity());
                if aux > AUX_JSON {
                    return false;
                }
            }
        }
        if aux > AUX_JSON {
            return false;
        }
        let Some(evicted) = small_evicted_entry(state) else {
            return false;
        };
        let charge = old
            .saturating_add(input)
            .saturating_add(aux)
            .saturating_mul(32)
            .saturating_add(evicted)
            .saturating_add((state.history.past.len() + 1) * size_of::<HistoryEntry>() * 2)
            .saturating_add(16 * 1024);
        if charge > LIMIT.saturating_sub(self.0) {
            return false;
        }
        self.0 += charge;
        true
    }
}
