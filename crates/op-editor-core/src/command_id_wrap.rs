//! Group/ungroup reuse canonical member tokens; only a wrapper is added/retired.
use super::{at, children_at, Edit, Entry, Index, Locations, Order, Pending, Scope, C};
use crate::{EditorState, PenNodeExt};
use jian_ops_schema::node::PenNode;
use std::{collections::HashSet, sync::Arc};
pub(super) fn supports(command: &C) -> bool {
    matches!(command, C::GroupSelected | C::UngroupSelected)
}
#[derive(Debug)]
pub(super) struct Wrap {
    group: Option<usize>,
    at: usize,
    positions: Vec<usize>,
    members: Vec<usize>,
}
impl Wrap {
    pub(super) fn pending(index: &Index, state: &EditorState, command: &C) -> Option<Pending> {
        let scope = state.command_lookup_active_scope();
        let root = index.root(&scope)?;
        let (parent, wrap) = match command {
            C::GroupSelected => {
                if state.selection.set.is_empty() || state.selection.set.len() > 4096 {
                    return None;
                }
                let mut parent: Option<usize> = None;
                let mut members = Vec::with_capacity(state.selection.set.len());
                let mut seen = HashSet::with_capacity(state.selection.set.len());
                let mut bytes = 0usize;
                for id in &state.selection.set {
                    bytes = bytes.checked_add(id.as_str().len())?;
                    if bytes > 256 * 1024 || !seen.insert(id.as_str()) {
                        return None;
                    }
                    let member = index.unique(root, id.as_str()).ok()??;
                    let current = index.entries[member].parent?;
                    if parent.is_some_and(|parent| parent != current) {
                        return None;
                    }
                    parent = Some(current);
                    members.push(member);
                }
                members.sort_unstable_by_key(|key| index.entries[*key].slot);
                let positions = members
                    .iter()
                    .map(|key| index.entries[*key].slot)
                    .collect::<Vec<_>>();
                (
                    parent?,
                    Self {
                        group: None,
                        at: positions[0],
                        positions,
                        members,
                    },
                )
            }
            C::UngroupSelected if state.selection.set.len() == 1 => {
                let group = index.unique(root, state.selection.anchor.as_str()).ok()??;
                if !at(&state.doc, &scope, &index.path(group))?.is_group() {
                    return None;
                }
                let row = &index.entries[group];
                if row.children.len() > 128 * 1024 {
                    return None;
                }
                (
                    row.parent?,
                    Self {
                        group: Some(group),
                        at: row.slot,
                        positions: Vec::new(),
                        members: row.children.clone(),
                    },
                )
            }
            _ => return None,
        };
        Some(Pending {
            scope,
            parent,
            len: index.entries[parent].children.len(),
            revision: state.document_revision(),
            command: Some(Box::new(command.clone())),
            edit: Edit::Wrap(wrap),
        })
    }
    pub(super) fn finish(
        &self,
        pending: &Pending,
        index: &mut Index,
        state: &EditorState,
    ) -> Option<()> {
        let nodes = children_at(&state.doc, &pending.scope, &index.path(pending.parent))?;
        if self.group.is_some() {
            self.ungroup(index, pending.parent, nodes)
        } else {
            self.group(index, pending.parent, nodes)
        }
    }
    fn group(&self, index: &mut Index, parent: usize, nodes: &[PenNode]) -> Option<()> {
        let old = &index.entries[parent].children;
        if nodes.len() != old.len().checked_sub(self.members.len())?.checked_add(1)? {
            return None;
        }
        let group = nodes.get(self.at)?;
        let inner = group.children().map_or(&[][..], Vec::as_slice);
        if !group.is_group()
            || inner.len() != self.members.len()
            || inner
                .iter()
                .zip(&self.members)
                .any(|(node, key)| node.id_str() != index.entries[*key].id.as_ref())
        {
            return None;
        }
        let mut expected = old
            .iter()
            .enumerate()
            .filter(|(slot, _)| self.positions.binary_search(slot).is_err());
        for (slot, node) in nodes.iter().enumerate() {
            if slot != self.at {
                let (_, key) = expected.next()?;
                if node.id_str() != index.entries[*key].id.as_ref() {
                    return None;
                }
            }
        }
        let key = index.shell(parent, group.id_str());
        index.entries[key].children = self.members.clone();
        let mut slot = 0;
        index.entries[parent].children.retain(|_| {
            let keep = self.positions.binary_search(&slot).is_err();
            slot += 1;
            keep
        });
        index.entries[parent].children.insert(self.at, key);
        for member in &self.members {
            index.entries[*member].parent = Some(key);
        }
        index.refresh_slots(key);
        index.refresh_slots(parent);
        Some(())
    }
    fn ungroup(&self, index: &mut Index, parent: usize, nodes: &[PenNode]) -> Option<()> {
        let group = self.group?;
        let old = &index.entries[parent].children;
        if nodes.len() != old.len().checked_sub(1)?.checked_add(self.members.len())? {
            return None;
        }
        for (slot, node) in nodes.iter().enumerate() {
            let key = if slot < self.at {
                old[slot]
            } else if slot < self.at + self.members.len() {
                self.members[slot - self.at]
            } else {
                old[slot - self.members.len() + 1]
            };
            if node.id_str() != index.entries[key].id.as_ref() {
                return None;
            }
        }
        drop(
            index.entries[parent]
                .children
                .splice(self.at..self.at + 1, self.members.iter().copied()),
        );
        for member in &self.members {
            index.entries[*member].parent = Some(parent);
        }
        let row = &mut index.entries[group];
        if index
            .ids
            .get_mut(&row.id)
            .is_some_and(|keys| keys.remove(group))
        {
            index.ids.remove(&row.id);
        }
        row.parent = None;
        row.id = Arc::from("");
        row.children.clear();
        index.refresh_slots(parent);
        Some(())
    }
}
impl Index {
    fn shell(&mut self, parent: usize, id: &str) -> usize {
        let id: Arc<str> = self
            .ids
            .get_key_value(id)
            .map_or_else(|| Arc::from(id), |(id, _)| Arc::clone(id));
        let key = self.entries.len();
        self.ids
            .entry(Arc::clone(&id))
            .and_modify(|keys| keys.add(key))
            .or_insert(Locations::One(key));
        self.entries.push(Entry {
            parent: Some(parent),
            slot: 0,
            root: self.entries[parent].root,
            id,
            children: Vec::new(),
        });
        key
    }
    fn refresh_slots(&mut self, parent: usize) {
        for slot in 0..self.entries[parent].children.len() {
            let key = self.entries[parent].children[slot];
            self.entries[key].slot = slot;
        }
    }
}
impl EditorState {
    pub(crate) fn command_selected_siblings(&self) -> Option<Option<(Box<[usize]>, Vec<usize>)>> {
        let scope = self.command_lookup_active_scope();
        let mut parent: Option<Box<[usize]>> = None;
        let mut positions = Vec::with_capacity(self.selection.set.len());
        let mut seen = HashSet::with_capacity(self.selection.set.len());
        for id in &self.selection.set {
            if !seen.insert(id.as_str()) {
                return Some(None);
            }
            let Some(path) = self.command_lookup_path(&scope, id, Order::SiblingFirst)? else {
                return Some(None);
            };
            let Some((&slot, current)) = path.split_last() else {
                return Some(None);
            };
            if parent.as_deref().is_some_and(|parent| parent != current) {
                return Some(None);
            }
            parent.get_or_insert_with(|| Box::<[usize]>::from(current));
            positions.push(slot);
        }
        positions.sort_unstable();
        Some(parent.map(|parent| (parent, positions)))
    }
}
