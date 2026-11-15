//! Exact header+append transactions. Complete real commands still execute;
//! these receipts retain only original changed header fields and splice slots.
use super::{bulk, EditorState, ScopedHistoryError, C};
use crate::{
    history::{Scope, TargetSpec},
    PenNodeExt,
};
use jian_ops_schema::{node::PenNode, PenDocument};
const LIMIT: usize = crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE;
#[derive(Debug)]
pub(super) struct Pending {
    fields: u8,
    bound: usize,
    targets: Vec<TargetSpec>,
    header: bulk::Pending,
}
struct Plan {
    fields: u8,
    bound: usize,
    targets: Vec<TargetSpec>,
}
fn members(command: &C) -> Option<Vec<&C>> {
    match command {
        C::InstantiateKitComponent { .. } => Some(vec![command]),
        C::Batch { commands } if !commands.is_empty() && commands.len() <= 4096 => {
            (!commands.iter().any(|c| matches!(c, C::Batch { .. })))
                .then(|| commands.iter().collect())
        }
        _ => None,
    }
}
fn fields(command: &C) -> Option<u8> {
    match command {
        C::SetVariables { .. }
        | C::InstantiateKitComponent {
            overrides_json: None,
            ..
        } => Some(1),
        C::SetThemes { .. } => Some(2),
        C::MergeThemePreset { .. } => Some(3),
        C::SetDesignMd { .. } => Some(4),
        C::InsertNode { .. }
        | C::BatchInsert { .. }
        | C::InsertSubtree { .. }
        | C::InsertAuthoredSubtree { .. }
        | C::InsertAuthoredSubtreePreservingRoots { .. } => Some(0),
        _ => None,
    }
}
fn add<T: serde::Serialize + ?Sized>(bytes: &mut usize, value: &T) -> Option<()> {
    *bytes = bytes.checked_add(
        crate::history_snapshot::command_capture_value_bytes(&value, LIMIT.checked_sub(*bytes)?)
            .ok()?,
    )?;
    (*bytes <= LIMIT).then_some(())
}
fn no_definitions(nodes: &[PenNode]) -> bool {
    let mut pending = vec![nodes.iter()];
    while let Some(siblings) = pending.last_mut() {
        if let Some(node) = siblings.next() {
            if matches!(node, PenNode::Frame(frame) if frame.reusable == Some(true)) {
                return false;
            }
            if let Some(children) = node.children() {
                pending.push(children.iter());
            }
        } else {
            pending.pop();
        }
    }
    true
}
fn plan(state: &EditorState, command: &C) -> Option<Plan> {
    if state.undo_capture_suspended
        || state.editor_ui.workspace.active
        || state.instance_write_virtual_anchor.is_some()
    {
        return None;
    }
    let commands = members(command)?;
    let mut mask = 0u8;
    let mut inserted = false;
    let mut bytes = 64 * 1024usize;
    for command in commands {
        mask |= fields(command)?;
        match command {
            C::SetVariables { variables, .. } => add(&mut bytes, variables)?,
            C::SetThemes { themes, .. } => add(&mut bytes, themes)?,
            C::MergeThemePreset { variables, themes } => {
                add(&mut bytes, variables)?;
                add(&mut bytes, themes)?;
            }
            C::SetDesignMd { spec } => add(&mut bytes, spec)?,
            C::InsertSubtree { nodes, .. }
            | C::InsertAuthoredSubtree { nodes, .. }
            | C::InsertAuthoredSubtreePreservingRoots { nodes, .. } => {
                if !no_definitions(nodes) {
                    return None;
                }
                inserted = true;
            }
            C::InsertNode { .. } | C::BatchInsert { .. } | C::InstantiateKitComponent { .. } => {
                inserted = true
            }
            _ => return None,
        }
    }
    if mask == 0 || !inserted {
        return None;
    }
    if mask & 1 != 0 {
        add(&mut bytes, &state.doc.variables)?;
    }
    if mask & 2 != 0 {
        add(&mut bytes, &state.doc.themes)?;
        add(&mut bytes, &state.ui.variables.active_theme)?;
    }
    if mask & 4 != 0 {
        add(&mut bytes, &state.doc.design_md)?;
    }
    let targets = state.mixed_append_targets(command)?;
    for target in &targets {
        let splice = target.splice.as_ref()?;
        // Replacements may remove registered definitions. Those remain on the
        // complete catalog path instead of inventing a weaker catalog inverse.
        if target.subtree || target.topology.is_some() || splice.remove != 0 {
            return None;
        }
        bytes = bytes.checked_add(splice.forecast)?;
        bytes = bytes.checked_add(target.path.len().checked_mul(64)?)?;
        if let Scope::Page { id, .. } = &target.scope {
            bytes = bytes.checked_add(id.len().checked_mul(8)?)?;
        }
        if bytes > LIMIT {
            return None;
        }
    }
    (!targets.is_empty()).then_some(Plan {
        fields: mask,
        bound: bytes,
        targets,
    })
}
pub(super) fn supports(command: &C) -> bool {
    members(command).is_some_and(|commands| {
        commands.iter().all(|command| fields(command).is_some())
            && commands
                .iter()
                .any(|command| fields(command).is_some_and(|mask| mask != 0))
            && commands.iter().any(|command| {
                matches!(
                    command,
                    C::InsertNode { .. }
                        | C::BatchInsert { .. }
                        | C::InsertSubtree { .. }
                        | C::InsertAuthoredSubtree { .. }
                        | C::InsertAuthoredSubtreePreservingRoots { .. }
                        | C::InstantiateKitComponent { .. }
                )
            })
    })
}
pub(super) fn forecast(state: &EditorState, command: &C) -> Option<usize> {
    plan(state, command).map(|plan| plan.bound)
}
impl Pending {
    pub(super) fn capture(
        state: &EditorState,
        command: &C,
    ) -> Option<(Self, Vec<Scope>, Vec<TargetSpec>)> {
        let plan = plan(state, command)?;
        let mut scopes = Vec::new();
        for target in &plan.targets {
            if !scopes.contains(&target.scope) {
                scopes.push(target.scope.clone());
            }
        }
        Some((
            Self {
                fields: plan.fields,
                bound: plan.bound,
                header: bulk::capture_fields(state, plan.fields),
                targets: plan.targets.clone(),
            },
            scopes,
            plan.targets,
        ))
    }
    pub(super) fn request_matches(&self, command: &C) -> bool {
        supports(command)
            && members(command).is_some_and(|commands| {
                commands
                    .into_iter()
                    .filter_map(fields)
                    .fold(0, |a, b| a | b)
                    == self.fields
            })
    }
    pub(super) fn targets_match(&self, state: &EditorState, command: &C) -> bool {
        plan(state, command).is_some_and(|plan| {
            plan.fields == self.fields && plan.bound <= self.bound && plan.targets == self.targets
        })
    }
    pub(super) fn matches_header(&self, doc: &PenDocument) -> bool {
        self.header.matches_header(doc)
    }
    pub(super) fn matches(&self, state: &EditorState) -> bool {
        self.header.matches(state)
    }
    pub(super) fn compact(
        self,
        state: &EditorState,
    ) -> Result<Option<bulk::Patch>, ScopedHistoryError> {
        self.header.compact(state)
    }
}
