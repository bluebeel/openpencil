//! Exact prepared clone/clipboard transactions. Only intended outputs and a
//! bounded input comparison stream are owned before the first live mutation.
use super::{
    patch::{self, Scope},
    splice::SplicePatch,
    ScopedHistoryError,
};
use crate::{EditorCommand as C, EditorState, PenNodeExt, SelectionState, SharedNode};
use std::{io::Write, sync::Arc};
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ClonePatch {
    pub groups: Vec<SplicePatch>,
}
impl ClonePatch {
    pub(super) fn bytes(&self) -> usize {
        self.groups.capacity() * std::mem::size_of::<SplicePatch>()
            + self
                .groups
                .iter()
                .map(SplicePatch::storage_bytes)
                .sum::<usize>()
    }
}
#[derive(Debug)]
struct ClipboardInput {
    wire: Vec<u8>,
    sensitivity: Box<[u8]>,
}
struct Bounded {
    bytes: Vec<u8>,
}
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len()
            > crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE.saturating_sub(self.bytes.len())
        {
            return Err(std::io::ErrorKind::OutOfMemory.into());
        }
        if self.bytes.capacity().saturating_sub(self.bytes.len()) < bytes.len() {
            let required = self.bytes.len() + bytes.len();
            let capacity = required
                .checked_next_power_of_two()
                .unwrap_or(required)
                .min(crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl ClipboardInput {
    fn capture(state: &EditorState) -> Option<Self> {
        let mut out = Bounded { bytes: Vec::new() };
        jian_ops_schema::image_table::without_save_scope(|| {
            serde_json::to_writer(&mut out, &state.clipboard)
        })
        .ok()?;
        Some(Self {
            wire: out.bytes,
            sensitivity: crate::history_snapshot::metadata_sensitivity(&state.clipboard),
        })
    }
    fn matches(&self, state: &EditorState) -> bool {
        struct Compare<'a> {
            expected: &'a [u8],
            at: usize,
        }
        impl Write for Compare<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.expected.get(self.at..self.at + bytes.len()) != Some(bytes) {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
                self.at += bytes.len();
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut compare = Compare {
            expected: &self.wire,
            at: 0,
        };
        jian_ops_schema::image_table::without_save_scope(|| {
            serde_json::to_writer(&mut compare, &state.clipboard)
        })
        .is_ok()
            && compare.at == self.wire.len()
            && self.sensitivity == crate::history_snapshot::metadata_sensitivity(&state.clipboard)
    }
}
#[derive(Debug)]
pub(super) struct PendingClones {
    scope: Scope,
    selection: SelectionState,
    paste: bool,
    offset: i32,
    high_water: u64,
    clipboard: Option<ClipboardInput>,
    prepared: Option<crate::mutators::clone_splice::Prepared>,
    traces: Vec<crate::mutators::clone_splice::Trace>,
    applied: bool,
}
impl PendingClones {
    pub(super) fn capture(state: &EditorState, scope: Scope, command: &C) -> Option<Self> {
        let (paste, offset) = match command {
            C::DuplicateSelected { offset_px } => (false, *offset_px),
            C::PasteClipboard { offset_px } => (true, *offset_px),
            _ => return None,
        };
        crate::mutators::clone_splice::forecast(state, command)?;
        let clipboard = if paste {
            Some(ClipboardInput::capture(state)?)
        } else {
            None
        };
        let prepared = crate::mutators::clone_splice::prepare_captured(state, command)?;
        let traces = prepared.traces(state.active_children());
        // Parent identities must be unambiguous before any mutation or broad
        // fallback. Numeric source routes are not an authority after rebasing.
        let mut wanted = super::identity::Wanted::new();
        for trace in &traces {
            if let Some(id) = &trace.parent_id {
                wanted.entry(scope.clone()).or_default().insert(id.clone());
            }
        }
        let index = super::identity::Index::new(&state.doc, wanted).ok()?;
        for trace in &traces {
            if let Some(id) = &trace.parent_id {
                if index.path(&scope, id).ok()?.as_ref() != trace.parent.as_ref() {
                    return None;
                }
            }
        }
        Some(Self {
            scope,
            selection: state.selection.clone(),
            paste,
            offset,
            high_water: state.runtime_id_high_water(),
            clipboard,
            prepared: Some(prepared),
            traces,
            applied: false,
        })
    }
    pub(super) fn request_matches(&self, state: &EditorState, command: &C) -> bool {
        let matches = match command {
            C::DuplicateSelected { offset_px } => !self.paste && self.offset == *offset_px,
            C::PasteClipboard { offset_px } => self.paste && self.offset == *offset_px,
            _ => false,
        };
        matches
            && self.selection == state.selection
            && self.high_water == state.runtime_id_high_water()
    }
    pub(super) fn matches(&self, state: &EditorState) -> bool {
        self.prepared.is_some()
            && self.selection == state.selection
            && self
                .clipboard
                .as_ref()
                .is_none_or(|input| input.matches(state))
    }
    pub(super) fn apply_direct(&mut self, state: &mut EditorState) -> bool {
        if !self.matches(state) {
            return false;
        }
        let ids = self
            .prepared
            .take()
            .unwrap()
            .commit(state.active_children_mut());
        if ids.is_empty() {
            return false;
        }
        state.selection = SelectionState {
            anchor: if self.paste {
                ids[0].clone()
            } else {
                ids.last().unwrap().clone()
            },
            set: ids,
        };
        let highest = self
            .traces
            .iter()
            .flat_map(|trace| trace.changed.iter())
            .filter_map(|id| crate::walkers::parse_n_id(id))
            .max()
            .unwrap_or(0);
        state.record_runtime_id_high_water(highest);
        state.mark_document_changed();
        self.applied = true;
        true
    }
    pub(super) fn compact(
        self,
        state: &EditorState,
    ) -> Result<Option<ClonePatch>, ScopedHistoryError> {
        if !self.applied {
            return Ok(None);
        }
        let mut wanted = super::identity::Wanted::new();
        for trace in &self.traces {
            if let Some(id) = &trace.parent_id {
                wanted
                    .entry(self.scope.clone())
                    .or_default()
                    .insert(id.clone());
            }
        }
        let index = super::identity::Index::new(&state.doc, wanted)?;
        let mut parents = std::collections::HashMap::new();
        for trace in &self.traces {
            if !parents.contains_key(&trace.parent_id) {
                let roots = patch::roots(&state.doc, &self.scope)?;
                let nodes = match &trace.parent_id {
                    Some(id) => patch::node(roots, &index.path(&self.scope, id)?)
                        .and_then(|node| node.children())
                        .ok_or(ScopedHistoryError::NodePathChanged)?
                        .as_slice(),
                    None => roots,
                };
                let mut positions = std::collections::HashMap::with_capacity(nodes.len());
                for (at, node) in nodes.iter().enumerate() {
                    match positions.entry(node.id_str().to_owned()) {
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(Some(at));
                        }
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            entry.insert(None);
                        }
                    }
                }
                parents.insert(trace.parent_id.clone(), (nodes.len(), positions));
            }
        }
        let mut groups = Vec::with_capacity(self.traces.len());
        for trace in self.traces {
            let (total, positions) = &parents[&trace.parent_id];
            groups.push(SplicePatch::from_trace(
                self.scope.clone(),
                trace,
                &index,
                *total,
                positions,
            )?);
        }
        Ok(Some(ClonePatch { groups }))
    }
    pub(super) fn restore(self, state: &mut EditorState) -> Result<(), ScopedHistoryError> {
        if let Some(clones) = self.compact(state)? {
            let mut patches = vec![patch::Patch::Clones(clones)];
            let catalog = super::splice_exchange::Catalog::new(&state.history, &patches);
            if let Some(prepared) =
                super::splice_exchange::Prepared::capture(&state.doc, &patches, &catalog, false)?
            {
                prepared.exchange(&mut state.doc, &mut patches);
            }
        }
        Ok(())
    }
}
