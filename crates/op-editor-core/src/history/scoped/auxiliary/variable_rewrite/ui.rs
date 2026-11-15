//! Exact cached binding edits; other actors' later bindings are never swapped.
use crate::{EditorState, NodeId};
use std::{collections::HashMap, mem};
#[derive(Debug)]
pub(super) struct Before {
    fill: Vec<(NodeId, String)>,
    stroke: Vec<(NodeId, String)>,
}
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    id: NodeId,
    old: Option<String>,
    expected: Option<String>,
    active: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Patch {
    fill: Vec<Entry>,
    stroke: Vec<Entry>,
}
impl Before {
    pub(super) fn capture(state: &EditorState, old: &str) -> Self {
        let capture = |map: &HashMap<NodeId, String>| {
            map.iter()
                .filter(|(_, value)| value.as_str() == old)
                .map(|(id, value)| (id.clone(), value.clone()))
                .collect()
        };
        Self {
            fill: capture(&state.ui.variables.fill_refs),
            stroke: capture(&state.ui.variables.stroke_refs),
        }
    }
    pub(super) fn matches(&self, state: &EditorState, old: &str) -> bool {
        fn matches(before: &[(NodeId, String)], map: &HashMap<NodeId, String>, old: &str) -> bool {
            before.len() == map.values().filter(|v| v.as_str() == old).count()
                && before.iter().all(|(id, value)| map.get(id) == Some(value))
        }
        matches(&self.fill, &state.ui.variables.fill_refs, old)
            && matches(&self.stroke, &state.ui.variables.stroke_refs, old)
    }
    pub(super) fn compact(self, state: &EditorState, new: Option<&str>) -> Option<Patch> {
        fn compact(
            before: Vec<(NodeId, String)>,
            map: &HashMap<NodeId, String>,
            new: Option<&str>,
        ) -> Option<Vec<Entry>> {
            before
                .into_iter()
                .map(|(id, value)| {
                    if map.get(&id).map(String::as_str) != new {
                        return None;
                    }
                    Some(Entry {
                        id,
                        old: Some(value),
                        expected: new.map(str::to_owned),
                        active: true,
                    })
                })
                .collect()
        }
        Some(Patch {
            fill: compact(self.fill, &state.ui.variables.fill_refs, new)?,
            stroke: compact(self.stroke, &state.ui.variables.stroke_refs, new)?,
        })
    }
    pub(super) fn forecast(state: &EditorState, name: &str, limit: usize) -> Option<usize> {
        let mut bytes = 0usize;
        for map in [
            &state.ui.variables.fill_refs,
            &state.ui.variables.stroke_refs,
        ] {
            for (id, value) in map {
                if value == name {
                    bytes = bytes.checked_add(
                        (mem::size_of::<Entry>() + id.as_str().len() + value.len() + 256)
                            .checked_mul(8)?,
                    )?;
                    if bytes > limit {
                        return None;
                    }
                }
            }
        }
        Some(bytes)
    }
}
impl Patch {
    pub(super) fn validate(&self, state: &EditorState) -> bool {
        let matches = |entries: &[Entry], map: &HashMap<NodeId, String>| {
            entries
                .iter()
                .all(|entry| map.get(&entry.id) == entry.expected.as_ref())
        };
        matches(&self.fill, &state.ui.variables.fill_refs)
            && matches(&self.stroke, &state.ui.variables.stroke_refs)
    }
    pub(super) fn exchange(&mut self, state: &mut EditorState) {
        fn exchange(patches: &mut [Entry], map: &mut HashMap<NodeId, String>) {
            for p in patches {
                if !p.active {
                    continue;
                }
                if map.get(&p.id) != p.expected.as_ref() {
                    p.active = false;
                    p.old = None;
                    p.expected = None;
                    continue;
                }
                p.old = match p.old.take() {
                    Some(old) => map.insert(p.id.clone(), old),
                    None => map.remove(&p.id),
                };
                p.expected = map.get(&p.id).cloned();
            }
        }
        exchange(&mut self.fill, &mut state.ui.variables.fill_refs);
        exchange(&mut self.stroke, &mut state.ui.variables.stroke_refs);
    }
    pub(super) fn bytes(&self) -> usize {
        let bytes = |v: &Vec<Entry>| {
            v.capacity() * mem::size_of::<Entry>()
                + v.iter()
                    .map(|e| {
                        e.id.as_str().len()
                            + e.old.as_ref().map_or(0, String::capacity)
                            + e.expected.as_ref().map_or(0, String::capacity)
                    })
                    .sum::<usize>()
        };
        mem::size_of::<Self>() + bytes(&self.fill) + bytes(&self.stroke)
    }
    pub(super) fn conflicts(&self, other: &Self) -> bool {
        self.fill
            .iter()
            .any(|a| other.fill.iter().any(|b| a.id == b.id))
            || self
                .stroke
                .iter()
                .any(|a| other.stroke.iter().any(|b| a.id == b.id))
    }
    pub(super) fn live_bound(&self, state: &EditorState, limit: usize) -> Option<usize> {
        let mut bytes = 0usize;
        for (entries, map) in [
            (&self.fill, &state.ui.variables.fill_refs),
            (&self.stroke, &state.ui.variables.stroke_refs),
        ] {
            for entry in entries {
                bytes = bytes.checked_add(
                    map.get(&entry.id)
                        .map_or(0, String::len)
                        .checked_mul(8)?
                        .checked_add(256)?,
                )?;
            }
        }
        (bytes <= limit).then_some(bytes)
    }
}
