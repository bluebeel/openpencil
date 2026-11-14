//! One catalog entry plus stable order anchors; never a whole catalog graph.
use super::super::{
    splice_order::{self, Anchors, Token},
    ScopedHistoryError,
};
use crate::{
    component_backing::DocumentNodeLocation, Component, EditorState, NodeId, SharedComponent,
    SharedNode,
};
use std::{mem, sync::Arc};
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub component: Arc<SharedComponent>,
    pub backing: Option<DocumentNodeLocation>,
}
pub(super) fn position(
    state: &EditorState,
    id: &NodeId,
) -> Result<Option<usize>, ScopedHistoryError> {
    let mut found = state
        .components
        .components
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.id == *id);
    let result = found.next().map(|(index, _)| index);
    if found.next().is_some() {
        return Err(ScopedHistoryError::NodePathChanged);
    }
    Ok(result)
}
impl Entry {
    pub(super) fn capture(
        state: &EditorState,
        id: &NodeId,
        anchor: Option<&Self>,
    ) -> Result<Option<Self>, ScopedHistoryError> {
        let Some(index) = position(state, id)? else {
            // A partial insertion/removal or public raw-vector mutation may
            // leave an orphan backing key. Do not certify an empty inverse;
            // rollback/recovery must fence and reload that uncertain state.
            if state.components.document_backing().contains_key(id) {
                return Err(ScopedHistoryError::NodePathChanged);
            }
            return Ok(None);
        };
        Ok(Some(Self {
            component: SharedComponent::capture(
                &state.components.components[index],
                anchor.map(|entry| &entry.component),
            ),
            backing: state.components.document_backing().get(id).cloned(),
        }))
    }
    pub(super) fn matches(&self, state: &EditorState, index: usize) -> bool {
        let entry = &state.components.components[index];
        self.component.id == entry.id
            && self.component.name == entry.name
            && self.component.root.equals_node(&entry.root)
            && self.backing.as_ref() == state.components.document_backing().get(&entry.id)
    }
    fn bytes(&self) -> usize {
        self.component.storage_bytes()
            + self.component.id.as_str().len()
            + self.component.name.capacity()
            + self.backing.as_ref().map_or(0, |location| {
                mem::size_of::<DocumentNodeLocation>()
                    + location.child_path.len() * mem::size_of::<usize>()
            })
    }
}
pub(super) fn anchors(state: &EditorState, id: &NodeId, index: usize) -> Anchors {
    Anchors {
        ids: vec![id.as_str().to_owned()],
        left: index
            .checked_sub(1)
            .and_then(|index| state.components.components.get(index))
            .map(|entry| entry.id.as_str().to_owned()),
        right: state
            .components
            .components
            .get(index + 1)
            .map(|entry| entry.id.as_str().to_owned()),
        required: false,
    }
}
pub(super) fn append_anchors(state: &EditorState, id: &NodeId) -> Anchors {
    Anchors {
        ids: vec![id.as_str().to_owned()],
        left: state
            .components
            .components
            .last()
            .map(|entry| entry.id.as_str().to_owned()),
        right: None,
        required: false,
    }
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Patch {
    pub id: NodeId,
    pub old: Option<Entry>,
    pub expected: Option<Entry>,
    pub anchor: Anchors,
    pub recent: Vec<Anchors>,
}
impl Patch {
    pub(super) fn compact(
        state: &EditorState,
        id: NodeId,
        old: Option<Entry>,
        anchor: Anchors,
    ) -> Result<Option<Self>, ScopedHistoryError> {
        let expected = Entry::capture(state, &id, old.as_ref())?;
        let same = match (&old, &expected) {
            (None, None) => true,
            (Some(a), Some(b)) => a == b,
            _ => false,
        };
        Ok((!same).then_some(Self {
            id,
            old,
            expected,
            anchor,
            recent: Vec::new(),
        }))
    }
    fn anchor(&self, required: bool) -> Anchors {
        let mut anchor = self.anchor.clone();
        anchor.required = required;
        anchor
    }
    /// Additional live comparison/materialization and stable-order scratch.
    /// Retained old/expected roots remain charged separately by the snapshot.
    pub(super) fn cohort_live_bound(&self, state: &EditorState, limit: usize) -> Option<usize> {
        fn add(total: &mut usize, bytes: usize, limit: usize) -> Option<()> {
            *total = total.checked_add(bytes)?;
            (*total <= limit).then_some(())
        }
        fn anchors(value: &Anchors) -> Option<usize> {
            let mut size = 256usize;
            for id in value
                .ids
                .iter()
                .chain(value.left.iter())
                .chain(value.right.iter())
            {
                size = size.checked_add(id.len().checked_mul(8)?)?;
            }
            Some(size)
        }
        let mut total = 128usize.checked_mul(1024)?;
        // Existing output rows, graph vertices/edges, and hash/order keys.
        for entry in &state.components.components {
            add(
                &mut total,
                512usize.checked_add(entry.id.as_str().len().checked_mul(8)?)?,
                limit,
            )?;
        }
        let backing = state.components.document_backing();
        if Arc::strong_count(&backing) > 2 {
            // A complete older checkpoint may share the live routing map.
            // Entry exchange clones that index only when it is still shared.
            for location in backing.values() {
                add(
                    &mut total,
                    location
                        .child_path
                        .len()
                        .checked_mul(mem::size_of::<usize>())?,
                    limit,
                )?;
            }
        }
        for snapshot in state
            .history
            .past
            .iter()
            .chain(&state.history.future)
            .filter_map(|entry| entry.scoped_snapshot())
        {
            for patch in &snapshot.auxiliary_patches {
                if let super::Patch::Catalog(patch) = patch {
                    add(&mut total, anchors(&patch.anchor)?, limit)?;
                    for recent in &patch.recent {
                        add(&mut total, anchors(recent)?, limit)?;
                    }
                }
            }
        }
        add(&mut total, anchors(&self.anchor)?, limit)?;
        for recent in &self.recent {
            add(&mut total, anchors(recent)?, limit)?;
        }
        if let Some(index) = position(state, &self.id).ok()? {
            let live = &state.components.components[index];
            let bytes = crate::history_snapshot::command_capture_node_bytes(
                &live.root,
                false,
                limit.checked_sub(total)?,
            )
            .ok()?;
            add(&mut total, bytes, limit)?;
            add(&mut total, live.name.len().checked_mul(8)?, limit)?;
        }
        if let Some(old) = &self.old {
            add(&mut total, old.component.name.len().checked_mul(8)?, limit)?;
            add(
                &mut total,
                old.component.id.as_str().len().checked_mul(8)?,
                limit,
            )?;
            if let Some(backing) = &old.backing {
                add(
                    &mut total,
                    backing
                        .child_path
                        .len()
                        .checked_mul(mem::size_of::<usize>())?,
                    limit,
                )?;
            }
            let bytes = crate::history_snapshot::command_capture_value_bytes(
                &old.component.root,
                limit.checked_sub(total)?,
            )
            .ok()?;
            add(&mut total, bytes, limit)?;
        }
        Some(total)
    }
    pub(super) fn roots(&self) -> impl Iterator<Item = &Arc<SharedNode>> {
        self.old
            .iter()
            .chain(self.expected.iter())
            .map(|entry| &entry.component.root)
    }
    pub(super) fn bytes(&self) -> usize {
        fn anchor(value: &Anchors) -> usize {
            value.ids.capacity() * mem::size_of::<String>()
                + value.ids.iter().map(String::capacity).sum::<usize>()
                + value.left.as_ref().map_or(0, String::capacity)
                + value.right.as_ref().map_or(0, String::capacity)
        }
        mem::size_of::<Self>()
            + self.id.as_str().len()
            + self.old.as_ref().map_or(0, Entry::bytes)
            + self.expected.as_ref().map_or(0, Entry::bytes)
            + anchor(&self.anchor)
            + self.recent.capacity() * mem::size_of::<Anchors>()
            + self.recent.iter().map(anchor).sum::<usize>()
    }
}
pub(super) struct Prepared {
    index: usize,
    present: bool,
    replacement: Option<Component>,
    backing: Option<DocumentNodeLocation>,
    recent: Vec<Anchors>,
}
impl Prepared {
    pub(super) fn capture(state: &EditorState, patch: &Patch) -> Result<Self, ScopedHistoryError> {
        let live = position(state, &patch.id)?;
        match (&patch.expected, live) {
            (Some(expected), Some(index)) if expected.matches(state, index) => {}
            (None, None) if !state.components.document_backing().contains_key(&patch.id) => {}
            _ => return Err(ScopedHistoryError::NodePathChanged),
        }
        let recent = live
            .map(|index| anchors(state, &patch.id, index))
            .into_iter()
            .collect();
        let index = if let Some(index) = live {
            index
        } else if patch.old.is_some() {
            let existing = state
                .components
                .components
                .iter()
                .map(|entry| entry.id.as_str().to_owned())
                .collect::<Vec<_>>();
            let mut known = Vec::new();
            for snapshot in state
                .history
                .past
                .iter()
                .chain(&state.history.future)
                .filter(|entry| {
                    !state
                        .history
                        .operation_conflicted(entry.history_tag().operation)
                })
                .filter_map(|entry| entry.scoped_snapshot())
                .filter(|snapshot| snapshot.generation == state.document_generation())
            {
                for edit in &snapshot.auxiliary_patches {
                    if let super::Patch::Catalog(entry) = edit {
                        known.push(entry.anchor(false));
                        known.extend(entry.recent.iter().cloned());
                    }
                }
            }
            known.push(patch.anchor(true));
            known.extend(patch.recent.iter().cloned());
            let order = splice_order::order(
                &existing,
                &[patch.id.as_str().to_owned()],
                known.into_iter(),
            )?;
            let index = order
                .iter()
                .position(|token| matches!(token, Token::Restore(0)))
                .ok_or(ScopedHistoryError::NodePathChanged)?;
            // The shared solver preserves every existing row's complete order.
            if order
                .iter()
                .filter_map(|token| match token {
                    Token::Existing(index) => Some(*index),
                    _ => None,
                })
                .ne(0..existing.len())
            {
                return Err(ScopedHistoryError::NodePathChanged);
            }
            index
        } else {
            return Err(ScopedHistoryError::NodePathChanged);
        };
        Ok(Self {
            index,
            present: live.is_some(),
            replacement: patch
                .old
                .as_ref()
                .map(|entry| entry.component.materialize()),
            backing: patch.old.as_ref().and_then(|entry| entry.backing.clone()),
            recent,
        })
    }
    pub(super) fn exchange(self, state: &mut EditorState, patch: &mut Patch) {
        state.components.exchange_catalog_entry(
            self.index,
            self.present,
            &patch.id,
            self.replacement,
            self.backing,
        );
        mem::swap(&mut patch.old, &mut patch.expected);
        patch.recent = self.recent;
    }
}
