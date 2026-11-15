//! Rename/delete use the compatibility mutator's exact metadata census.
use super::{exact, EditorState, MapPatch, ScopedHistoryError, C};
use crate::{
    history::{Scope, TargetSpec},
    variables_resolve::reference_index::Route,
    PenNodeExt,
};
use jian_ops_schema::{variable::VariableDefinition, PenDocument};
use std::sync::Arc;
mod ui;
const LIMIT: usize = crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE;
#[derive(Debug)]
pub(super) struct Pending {
    old: String,
    new: Option<String>,
    keys: Vec<(String, Option<VariableDefinition>)>,
    present: bool,
    total: usize,
    ui: ui::Before,
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Patch {
    variables: MapPatch<VariableDefinition>,
    ui: ui::Patch,
}
pub(super) struct Plan {
    pub pending: Pending,
    pub scopes: Vec<Scope>,
    pub targets: Vec<TargetSpec>,
}
pub(super) fn supports(command: &C) -> bool {
    matches!(command, C::RenameVariable { .. } | C::DeleteVariable { .. })
}
fn request<'a>(state: &EditorState, command: &'a C) -> Option<(&'a str, Option<&'a str>)> {
    let vars = state.doc.variables.as_ref()?;
    match command {
        C::DeleteVariable { name } if vars.contains_key(name) => Some((name, None)),
        C::RenameVariable { old_name, new_name } => {
            let new = new_name.trim();
            if new.is_empty()
                || new == old_name
                || !vars.contains_key(old_name)
                || vars.contains_key(new)
            {
                None
            } else {
                Some((old_name, Some(new)))
            }
        }
        _ => None,
    }
}
fn targets(state: &EditorState, name: &str) -> Option<Vec<Arc<Route>>> {
    let routes = state.variable_reference_routes(name)?;
    if !routes.is_empty() {
        let mut definitions = std::collections::HashSet::new();
        for component in &state.components.components {
            if !state.components.is_document_backed(&component.id) {
                return None;
            }
            definitions.insert(component.id.as_str());
        }
        for route in &routes {
            let node = route.node(&state.doc)?;
            if definitions.contains(node.id_str())
                || matches!(node,jian_ops_schema::node::PenNode::Frame(frame)if frame.reusable==Some(true))
                || !crate::NodeId::new(route.id.clone()).is_real()
            {
                return None;
            }
        }
    }
    Some(routes)
}
pub(super) fn forecast(state: &EditorState, command: &C) -> Option<usize> {
    if state.editor_ui.workspace.active
        || state.instance_write_virtual_anchor.is_some()
        || state.undo_capture_suspended
    {
        return None;
    }
    let (old, new) = request(state, command)?;
    let routes = targets(state, old)?;
    let mut bytes = 64 * 1024usize;
    for name in std::iter::once(old).chain(new) {
        bytes = bytes.checked_add(name.len().checked_mul(8)?)?.checked_add(
            crate::history_snapshot::command_capture_value_bytes(
                &state.doc.variables.as_ref()?.get(name),
                LIMIT.checked_sub(bytes)?,
            )
            .ok()?,
        )?;
    }
    bytes = bytes.checked_add(ui::Before::forecast(state, old, LIMIT.checked_sub(bytes)?)?)?;
    for route in routes {
        bytes = bytes
            .checked_add(
                crate::history_snapshot::command_capture_node_bytes(
                    route.node(&state.doc)?,
                    true,
                    LIMIT.checked_sub(bytes)?,
                )
                .ok()?,
            )?
            .checked_add(
                route
                    .path
                    .len()
                    .checked_mul(64)?
                    .checked_add(route.id.len().checked_mul(8)?)?,
            )?;
        if bytes > LIMIT {
            return None;
        }
    }
    (bytes <= LIMIT).then_some(bytes)
}
impl Pending {
    pub(super) fn capture(state: &EditorState, command: &C) -> Option<Plan> {
        forecast(state, command)?;
        let (old, new) = request(state, command)?;
        let routes = targets(state, old)?;
        let mut scopes = Vec::new();
        let mut targets = Vec::with_capacity(routes.len());
        for route in routes {
            if !scopes.contains(&route.scope) {
                scopes.push(route.scope.clone());
            }
            targets.push(TargetSpec {
                scope: route.scope.clone(),
                path: route.path.clone(),
                subtree: false,
                splice: None,
                topology: None,
            });
        }
        let keys = std::iter::once(old)
            .chain(new)
            .map(|name| {
                (
                    name.into(),
                    state
                        .doc
                        .variables
                        .as_ref()
                        .and_then(|m| m.get(name))
                        .cloned(),
                )
            })
            .collect();
        Some(Plan {
            pending: Self {
                old: old.into(),
                new: new.map(str::to_owned),
                keys,
                present: state.doc.variables.is_some(),
                total: state.doc.variables.as_ref().map_or(0, |m| m.len()),
                ui: ui::Before::capture(state, old),
            },
            scopes,
            targets,
        })
    }
    pub(super) fn matches_header(&self, doc: &PenDocument) -> bool {
        self.present == doc.variables.is_some()
            && self.total == doc.variables.as_ref().map_or(0, |m| m.len())
            && self.keys.iter().all(|(key, value)| {
                exact(
                    &value.as_ref(),
                    &doc.variables.as_ref().and_then(|m| m.get(key)),
                )
            })
    }
    pub(super) fn matches(&self, state: &EditorState) -> bool {
        self.matches_header(&state.doc) && self.ui.matches(state, &self.old)
    }
    pub(super) fn request_matches(&self, command: &C) -> bool {
        match command {
            C::DeleteVariable { name } => self.new.is_none() && name == &self.old,
            C::RenameVariable { old_name, new_name } => {
                old_name == &self.old && self.new.as_deref() == Some(new_name.trim())
            }
            _ => false,
        }
    }
    pub(super) fn compact(self, state: &EditorState) -> Result<Option<Patch>, ScopedHistoryError> {
        let Some(variables) =
            MapPatch::selected_keys(self.present, self.total, self.keys, &state.doc.variables)
        else {
            // Refusal or an unwind before definition mutation leaves no key
            // delta. Changed node checkpoints still own any completed prefix.
            return if self.ui.matches(state, &self.old) {
                Ok(None)
            } else {
                Err(ScopedHistoryError::DocumentHeaderChanged)
            };
        };
        let ui = self
            .ui
            .compact(state, self.new.as_deref())
            .ok_or(ScopedHistoryError::DocumentHeaderChanged)?;
        if !variables.validate(&state.doc.variables) {
            return Err(ScopedHistoryError::DocumentHeaderChanged);
        }
        Ok(Some(Patch { variables, ui }))
    }
}
impl Patch {
    pub(super) fn variables(&self) -> &MapPatch<VariableDefinition> {
        &self.variables
    }
    pub(super) fn actor_safe(&self) -> bool {
        self.variables.actor_safe()
    }
    pub(super) fn conflicts(&self, other: &Self) -> bool {
        self.variables.actor_conflicts(&other.variables) || self.ui.conflicts(&other.ui)
    }
    pub(super) fn validate(&self, state: &EditorState, actor: bool) -> bool {
        if actor {
            self.variables.validate_actor(&state.doc.variables)
        } else {
            self.variables.validate(&state.doc.variables) && self.ui.validate(state)
        }
    }
    pub(super) fn exchange(&mut self, state: &mut EditorState) {
        self.variables.exchange(&mut state.doc.variables);
        self.ui.exchange(state);
    }
    pub(super) fn bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.variables.storage_bytes() + self.ui.bytes()
    }
    pub(super) fn live_bound(&self, state: &EditorState, limit: usize) -> Option<usize> {
        let a = self
            .variables
            .cohort_live_bound(&state.doc.variables, limit)?;
        a.checked_add(self.ui.live_bound(state, limit.checked_sub(a)?)?)
    }
}
