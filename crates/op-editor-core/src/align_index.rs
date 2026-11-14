//! One read pass and one mutation pass for large align/distribute selections.
use super::{AlignAction, DocRect};
use crate::geometry::own_bounds;
use crate::{EditorState, NodeId, PenNodeExt};
use jian_ops_schema::node::PenNode;
use std::collections::{HashMap, HashSet};

type Deltas = HashMap<String, (f64, f64)>;

#[derive(Clone, Copy)]
struct Entry {
    bounds: DocRect,
    editable: bool,
    movable: bool,
}

// Keep extrema, rather than repeatedly unioning already-subtracted extents:
// union_of performs exactly this arithmetic in sibling order.
#[derive(Default)]
struct Extrema(Option<(f64, f64, f64, f64)>);
impl Extrema {
    fn add(&mut self, rect: DocRect) {
        if rect.is_empty() {
            return;
        }
        let current = (rect.x, rect.y, rect.x + rect.w, rect.y + rect.h);
        self.0 = Some(match self.0 {
            None => current,
            Some((x, y, right, bottom)) => (
                x.min(current.0),
                y.min(current.1),
                right.max(current.2),
                bottom.max(current.3),
            ),
        });
    }
    fn bounds(self) -> DocRect {
        self.0
            .map_or(DocRect::ZERO, |(x, y, right, bottom)| DocRect {
                x,
                y,
                w: right - x,
                h: bottom - y,
            })
    }
}

struct Frame<'a> {
    node: Option<&'a PenNode>,
    children: std::slice::Iter<'a, PenNode>,
    union: Extrema,
    ancestor_selected: bool,
    selected: bool,
    editable: bool,
    parent_flex: bool,
}

fn collect<'a>(roots: &'a [PenNode], wanted: &HashSet<&str>) -> Option<HashMap<&'a str, Entry>> {
    let mut entries = HashMap::with_capacity(wanted.len());
    let mut seen = HashSet::with_capacity(wanted.len());
    let mut stack = vec![Frame {
        node: None,
        children: roots.iter(),
        union: Extrema::default(),
        ancestor_selected: false,
        selected: false,
        editable: false,
        parent_flex: false,
    }];
    while let Some(parent) = stack.last_mut() {
        if let Some(node) = parent.children.next() {
            let selected = wanted.contains(node.id_str());
            if selected && !seen.insert(node.id_str()) {
                return None;
            }
            let editable = selected
                && node.base().visible.unwrap_or(true)
                && !node.base().locked.unwrap_or(false);
            let ancestor_selected = parent.ancestor_selected || parent.editable;
            let parent_flex = parent
                .node
                .is_some_and(PenNodeExt::is_auto_layout_container);
            stack.push(Frame {
                node: Some(node),
                children: node.children().map_or(&[][..], Vec::as_slice).iter(),
                union: Extrema::default(),
                ancestor_selected,
                selected,
                editable,
                parent_flex,
            });
        } else {
            let frame = stack.pop().expect("the current frame exists");
            let Some(node) = frame.node else {
                break;
            };
            let own = own_bounds(node);
            let bounds = if own.w > 0.0 || own.h > 0.0 {
                own
            } else {
                frame.union.bounds()
            };
            if frame.selected {
                entries.insert(
                    node.id_str(),
                    Entry {
                        bounds,
                        editable: frame.editable,
                        movable: frame.editable && !frame.ancestor_selected && !frame.parent_flex,
                    },
                );
            }
            if let Some(parent) = stack.last_mut() {
                parent.union.add(bounds);
            }
        }
    }
    Some(entries)
}

/// None selects the existing identity-sensitive fallback. An empty plan is a
/// complete no-op and avoids even capturing an undo snapshot.
pub(super) fn plan(state: &EditorState, action: AlignAction) -> Option<Deltas> {
    if state.instance_write_virtual_anchor.is_some() {
        return None;
    }
    let wanted: HashSet<&str> = state.selection.set.iter().map(NodeId::as_str).collect();
    if wanted.len() != state.selection.set.len() {
        return None;
    }
    if wanted.is_empty() {
        return Some(Deltas::new());
    }
    let entries = collect(state.active_children(), &wanted)?;
    let editable = state
        .selection
        .set
        .iter()
        .filter_map(|id| {
            entries
                .get(id.as_str())
                .filter(|entry| entry.editable)
                .map(|entry| (id, *entry))
        })
        .collect::<Vec<_>>();
    let mut deltas = Deltas::new();
    if editable.is_empty() || (action.is_distribute() && editable.len() < 3) {
        return Some(deltas);
    }
    let reference = if editable.len() >= 2 {
        let mut union = Extrema::default();
        for (_, entry) in &editable {
            union.add(entry.bounds);
        }
        let bounds = union.bounds();
        if bounds.is_empty() {
            return Some(deltas);
        }
        bounds
    } else {
        let Some(bounds) = super::parent_aggregate_bounds(state.active_children(), editable[0].0)
        else {
            return Some(deltas);
        };
        bounds
    };
    let mut targets = editable
        .into_iter()
        .filter(|(_, entry)| entry.movable)
        .collect::<Vec<_>>();
    if action.is_distribute() {
        if targets.len() < 3 {
            return Some(deltas);
        }
        let center = |rect: DocRect| {
            if action.is_horizontal() {
                rect.x + rect.w / 2.0
            } else {
                rect.y + rect.h / 2.0
            }
        };
        targets.sort_by(|a, b| {
            center(a.1.bounds)
                .partial_cmp(&center(b.1.bounds))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let count = targets.len();
        let first = center(targets[0].1.bounds);
        let step = (center(targets[count - 1].1.bounds) - first) / (count - 1) as f64;
        for (index, (id, entry)) in targets.iter().enumerate().take(count - 1).skip(1) {
            let delta = first + step * index as f64 - center(entry.bounds);
            if delta != 0.0 {
                deltas.insert(
                    id.as_str().into(),
                    if action.is_horizontal() {
                        (delta, 0.0)
                    } else {
                        (0.0, delta)
                    },
                );
            }
        }
    } else {
        for (id, entry) in targets {
            let b = entry.bounds;
            let delta = match action {
                AlignAction::Left => (reference.x - b.x, 0.0),
                AlignAction::CenterH => (reference.x + reference.w / 2.0 - (b.x + b.w / 2.0), 0.0),
                AlignAction::Right => (reference.x + reference.w - (b.x + b.w), 0.0),
                AlignAction::Top => (0.0, reference.y - b.y),
                AlignAction::CenterV => (0.0, reference.y + reference.h / 2.0 - (b.y + b.h / 2.0)),
                AlignAction::Bottom => (0.0, reference.y + reference.h - (b.y + b.h)),
                _ => unreachable!("distribution is handled above"),
            };
            if delta != (0.0, 0.0) {
                deltas.insert(id.as_str().into(), delta);
            }
        }
    }
    Some(deltas)
}

pub(super) fn apply(roots: &mut [PenNode], mut deltas: Deltas) -> bool {
    fn walk(nodes: &mut [PenNode], deltas: &mut Deltas) -> bool {
        let mut moved = false;
        for node in nodes {
            if deltas.is_empty() {
                break;
            }
            if let Some((dx, dy)) = deltas.remove(node.id_str()) {
                crate::walkers::translate_subtree(node, dx, dy);
                moved = true;
                // Selected descendants were excluded during planning.
                continue;
            }
            if node.children().is_some() {
                if let Some(children) = node.children_mut() {
                    moved |= walk(children, deltas);
                }
            }
        }
        moved
    }
    walk(roots, &mut deltas)
}
