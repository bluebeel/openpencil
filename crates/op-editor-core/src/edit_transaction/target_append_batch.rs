//! Static authoring retains original append slots and touched headers, not a
//! containing page per leaf. Every real command still sees the complete Doc.
use super::{command_scope, splice, Scope, TargetSpec};
use crate::{EditorCommand as C, EditorState, NodeId, PenNodeExt};
use jian_ops_schema::node::PenNode;
use std::collections::HashMap;
const LIMIT: usize = crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE;
#[derive(Clone)]
struct Owned {
    anchor: usize,
    container: bool,
    reference: bool,
}
struct Plan<'a> {
    state: &'a EditorState,
    active: usize,
    targets: Vec<TargetSpec>,
    owned: HashMap<(Scope, String), Owned>,
    steps: usize,
    appended: bool,
    groups: HashMap<(Scope, Option<String>), usize>,
    destinations: HashMap<usize, splice::DestinationBudget>,
}
impl Plan<'_> {
    fn scope(&self, raw: Option<&str>) -> Option<Scope> {
        Some(command_scope::scope(
            self.state,
            command_scope::page_index(self.state, raw, self.active)?,
        ))
    }
    fn add_bound(&mut self, at: usize, bytes: usize) -> Option<()> {
        let splice = self.targets.get_mut(at)?.splice.as_mut()?;
        splice.forecast = splice.forecast.checked_add(bytes)?;
        (splice.forecast <= LIMIT).then_some(())
    }
    fn merge(&mut self, spec: TargetSpec, key: (Scope, Option<String>)) -> Option<usize> {
        let edit = spec.splice.as_ref()?;
        if let Some(&at) = self.groups.get(&key) {
            let shape = *self.destinations.get(&at)?;
            let old = self.targets[at].splice.as_mut()?;
            let old_bound = shape.bound(old.insert_limit)?;
            if old.remove > 1 || edit.remove > 1 {
                return None;
            }
            if old.remove == 0 && edit.remove == 1 {
                old.index = edit.index;
                old.remove = 1;
            }
            if old.remove == 1 && edit.remove == 1 && old.index != edit.index {
                return None;
            }
            let new_limit = old.insert_limit.checked_add(edit.insert_limit)?;
            old.forecast = old
                .forecast
                .checked_sub(old_bound)?
                .checked_add(edit.forecast.checked_sub(shape.bound(edit.insert_limit)?)?)?
                .checked_add(shape.bound(new_limit)?)?;
            old.insert_limit = new_limit;
            if new_limit > 4096 || old.forecast > LIMIT {
                return None;
            }
            return Some(at);
        }
        let shape = splice::destination_shape(self.state, &spec.scope, &spec.path)?;
        let at = self.targets.len();
        self.destinations.insert(at, shape);
        self.groups.insert(key, at);
        self.targets.push(spec);
        Some(at)
    }
    fn extend_group(&mut self, at: usize, count: usize, bound: usize) -> Option<()> {
        let shape = *self.destinations.get(&at)?;
        let edit = self.targets.get_mut(at)?.splice.as_mut()?;
        let old = shape.bound(edit.insert_limit)?;
        let next = edit.insert_limit.checked_add(count)?;
        if next > 4096 {
            return None;
        }
        edit.forecast = edit
            .forecast
            .checked_sub(old)?
            .checked_add(bound)?
            .checked_add(shape.bound(next)?)?;
        edit.insert_limit = next;
        (edit.forecast <= LIMIT).then_some(())
    }
    fn insert(&mut self, command: &C) -> Option<()> {
        let (parent, raw) = match command {
            C::InsertNode {
                target_parent,
                page_id,
                ..
            } => (Some(target_parent), page_id.as_deref()),
            C::InsertSubtree {
                parent_id, page_id, ..
            }
            | C::InsertAuthoredSubtree {
                parent_id, page_id, ..
            }
            | C::InsertAuthoredSubtreePreservingRoots {
                parent_id, page_id, ..
            } => (Some(parent_id), page_id.as_deref()),
            C::BatchInsert { page_id, .. } => (None, page_id.as_deref()),
            _ => return None,
        };
        let scope = self.scope(raw)?;
        let owned = parent
            .filter(|parent| parent.is_real())
            .and_then(|parent| self.owned.get(&(scope.clone(), parent.as_str().to_owned())))
            .cloned();
        let key = (
            scope.clone(),
            parent
                .filter(|parent| parent.is_real())
                .map(|parent| parent.as_str().to_owned()),
        );
        let count = match command {
            C::InsertSubtree { nodes, .. }
            | C::InsertAuthoredSubtree { nodes, .. }
            | C::InsertAuthoredSubtreePreservingRoots { nodes, .. } => nodes.len(),
            C::BatchInsert { items, .. } => items.len(),
            _ => 1,
        };
        let replace = matches!(command,C::InsertSubtree{nodes,..}|C::InsertAuthoredSubtree{nodes,..} if nodes.len()==1&&matches!(nodes[0],PenNode::Frame(_)));
        let existing = self.groups.get(&key).copied().filter(|at| {
            !(key.1.is_none()
                && replace
                && self
                    .destinations
                    .get(at)
                    .is_some_and(|shape| shape.empty_root)
                && self.targets[*at]
                    .splice
                    .as_ref()
                    .is_some_and(|edit| edit.remove == 0))
        });
        let anchor = if let Some(parent) = owned {
            if !parent.container || parent.reference {
                return None;
            }
            self.add_bound(parent.anchor, splice::insertion_bound(command)?)?;
            parent.anchor
        } else if let Some(at) = existing {
            self.extend_group(at, count, splice::insertion_bound(command)?)?;
            at
        } else {
            let spec = splice::plan(self.state, command, self.active)?;
            self.merge(spec, key)?
        };
        if let C::InsertAuthoredSubtree { nodes, .. }
        | C::InsertAuthoredSubtreePreservingRoots { nodes, .. } = command
        {
            let mut pending: Vec<_> = nodes.iter().map(|node| (node, false)).collect();
            while let Some((node, parent_reference)) = pending.pop() {
                if node.id_str().is_empty() || self.owned.len() >= 16_384 {
                    return None;
                }
                let reference = parent_reference || matches!(node, PenNode::Ref(_));
                if self
                    .owned
                    .insert(
                        (scope.clone(), node.id_str().to_owned()),
                        Owned {
                            anchor,
                            container: node.is_container(),
                            reference,
                        },
                    )
                    .is_some()
                {
                    return None;
                }
                if let Some(children) = node.children() {
                    pending.extend(children.iter().map(|child| (child, reference)));
                }
            }
        }
        self.appended = true;
        Some(())
    }
    fn leaf(&mut self, command: &C) -> Option<()> {
        self.steps += 1;
        if self.steps > 4096 {
            return None;
        }
        if matches!(
            command,
            C::InsertNode { .. }
                | C::BatchInsert { .. }
                | C::InsertSubtree { .. }
                | C::InsertAuthoredSubtree { .. }
                | C::InsertAuthoredSubtreePreservingRoots { .. }
        ) {
            return self.insert(command);
        }
        if let C::DeleteNode { node_id, page_id } = command {
            let owned = self
                .owned
                .get(&(self.scope(page_id.as_deref())?, node_id.as_str().to_owned()))?;
            return (!owned.reference).then_some(());
        }
        if let C::MoveNode {
            node_id,
            target_parent,
            index,
            page_id,
        } = command
        {
            let scope = self.scope(page_id.as_deref())?;
            let source = self
                .owned
                .get(&(scope.clone(), node_id.as_str().to_owned()))?
                .clone();
            if source.reference {
                return None;
            }
            if let Some(parent) = self
                .owned
                .get(&(scope.clone(), target_parent.as_str().to_owned()))
            {
                return (parent.container && !parent.reference).then_some(());
            }
            let mut spec = splice::append_destination(self.state, &scope, target_parent)?;
            let extra = self
                .targets
                .iter()
                .filter(|old| old.scope == scope && old.path == spec.path)
                .filter_map(|old| old.splice.as_ref())
                .map(|old| old.insert_limit)
                .sum::<usize>();
            let initial = spec.splice.as_ref()?.index;
            if index.is_some_and(|index| index < initial.saturating_add(extra)) {
                return None;
            }
            spec.splice.as_mut()?.forecast =
                64 * 1024 + splice::destination_bound(self.state, &scope, &spec.path, 1)?;
            self.merge(
                spec,
                (
                    scope.clone(),
                    target_parent
                        .is_real()
                        .then(|| target_parent.as_str().to_owned()),
                ),
            )?;
            self.add_bound(source.anchor, 64 * 1024)?;
            return Some(());
        }
        if let Some((id, raw)) = metadata_id(command) {
            let scope = self.scope(raw)?;
            if let Some(owned) = self.owned.get(&(scope, id.as_str().to_owned())) {
                if owned.reference || !header_only(command) {
                    return None;
                }
                return Some(());
            }
        }
        super::collect(
            self.state,
            command,
            &mut self.active,
            &mut self.targets,
            true,
            false,
        )
    }
}
fn metadata_id(command: &C) -> Option<(&NodeId, Option<&str>)> {
    match command {
        C::UpdateNode {
            node_id, page_id, ..
        }
        | C::PatchNodeData {
            node_id, page_id, ..
        } => Some((node_id, page_id.as_deref())),
        C::SetNodeFlag { node_id, .. }
        | C::SetNodeFlip { node_id, .. }
        | C::SetEllipseArc { node_id, .. }
        | C::AddNodeEffect { node_id, .. }
        | C::RemoveNodeEffect { node_id, .. }
        | C::SetEffectParam { node_id, .. }
        | C::SetEffectColor { node_id, .. }
        | C::SetNodeRotation { node_id, .. }
        | C::SetNodeText { node_id, .. }
        | C::SetNodeCornerRadius { node_id, .. }
        | C::SetNodeFontSize { node_id, .. }
        | C::SetNodeFontWeight { node_id, .. }
        | C::SetNodeStrokeHex { node_id, .. }
        | C::SetNodeStrokeWidth { node_id, .. }
        | C::SetNodeStrokeSideWidth { node_id, .. }
        | C::SetNodeFillHex { node_id, .. }
        | C::SetNodeName { node_id, .. }
        | C::SetImageVideoSrc { node_id, .. }
        | C::SetImageVideoPlayback { node_id, .. }
        | C::AddImageVideo { node_id, .. }
        | C::RemoveImageVideo { node_id, .. }
        | C::SetNodeLayoutProp { node_id, .. } => Some((node_id, None)),
        _ => None,
    }
}
fn header_only(command: &C) -> bool {
    match command {
        C::PatchNodeData { patch_json, .. } => {
            serde_json::from_str::<serde_json::Value>(patch_json)
                .ok()
                .and_then(|value| {
                    value.as_object().map(|object| {
                        !["children", "id", "type"]
                            .iter()
                            .any(|key| object.contains_key(*key))
                    })
                })
                .unwrap_or(false)
        }
        _ => metadata_id(command).is_some(),
    }
}
pub(super) fn plan(state: &EditorState, command: &C) -> Option<Vec<TargetSpec>> {
    let C::Batch { commands } = command else {
        return None;
    };
    let mut plan = Plan {
        state,
        active: state.ui.active_page_index,
        targets: Vec::new(),
        owned: HashMap::new(),
        steps: 0,
        appended: false,
        groups: HashMap::new(),
        destinations: HashMap::new(),
    };
    let mut pending: Vec<_> = commands
        .iter()
        .rev()
        .map(|command| (command, 0usize))
        .collect();
    while let Some((command, depth)) = pending.pop() {
        if depth > 32 {
            return None;
        }
        if let C::Batch { commands } = command {
            pending.extend(commands.iter().rev().map(|command| (command, depth + 1)));
        } else {
            plan.leaf(command)?;
        }
    }
    if !plan.appended {
        return None;
    }
    let splices: Vec<_> = plan
        .targets
        .iter()
        .filter(|target| target.splice.is_some())
        .cloned()
        .collect();
    plan.targets.retain(|target| {
        target.splice.is_some() || !splices.iter().any(|splice| removed_covers(splice, target))
    });
    for target in &plan.targets {
        if target.splice.is_none()
            && splices.iter().any(|splice| {
                splice.scope == target.scope
                    && splice.path == target.path
                    && splice.splice.as_ref().is_some_and(|edit| edit.remove == 0)
            })
        {
            let roots = scope_roots(state, &target.scope)?;
            let node = node(roots, &target.path)?;
            if node.children().is_none() {
                return None;
            }
        }
    }
    Some(plan.targets)
}
fn removed_covers(splice: &TargetSpec, target: &TargetSpec) -> bool {
    let Some(edit) = &splice.splice else {
        return false;
    };
    if splice.scope != target.scope || edit.remove == 0 || !target.path.starts_with(&splice.path) {
        return false;
    }
    target
        .path
        .get(splice.path.len())
        .is_some_and(|at| *at >= edit.index && *at < edit.index + edit.remove)
}
fn scope_roots<'a>(state: &'a EditorState, scope: &Scope) -> Option<&'a [PenNode]> {
    match scope {
        Scope::Root => Some(&state.doc.children),
        Scope::Page { index, .. } => Some(&state.doc.pages.as_ref()?.get(*index)?.children),
    }
}
fn node<'a>(mut roots: &'a [PenNode], path: &[usize]) -> Option<&'a PenNode> {
    let (last, parents) = path.split_last()?;
    for at in parents {
        roots = roots.get(*at)?.children()?;
    }
    roots.get(*last)
}
