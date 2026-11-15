//! Exact key/name inverses for audited no-repair host commands. Generic
//! document-header and arbitrary callback captures remain complete.
use super::{
    header_map::{exact, MapPatch},
    Scope, ScopedHistoryError,
};
use crate::{EditorCommand as C, EditorState, NodeId};
use jian_ops_schema::variable::VariableDefinition;
use std::mem;
mod bulk;
#[path = "auxiliary/catalog.rs"]
mod catalog;
#[path = "auxiliary/forward.rs"]
mod forward;
#[path = "auxiliary/lifecycle.rs"]
mod lifecycle;
mod mixed;
mod save;
mod upsert;
mod variable_rewrite;
pub(super) use forward::Label as ForwardLabel;
#[path = "auxiliary/page_catalog.rs"]
mod page_catalog;
pub(super) use page_catalog::{AddedPage, PreparedPageCatalog};
#[path = "auxiliary_plan.rs"]
mod plan;
#[derive(Debug)]
pub(super) enum Target {
    Upsert(upsert::Pending),
    VariableRewrite(variable_rewrite::Pending),
    Bulk(bulk::Pending),
    Mixed(mixed::Pending),
    Lifecycle(lifecycle::Pending),
    Variable {
        key: String,
        old: Option<VariableDefinition>,
        present: bool,
        total: usize,
        create: bool,
    },
    Page {
        index: usize,
        id: String,
        old: String,
    },
    Component {
        index: usize,
        id: NodeId,
        old: String,
    },
}
#[derive(Debug)]
pub(super) struct Pending {
    value: Target,
}
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Patch {
    ConversionUnit(bulk::conversion::Patch),
    VariableRewrite(variable_rewrite::Patch),
    Bulk(bulk::Patch),
    Catalog(catalog::Patch),
    Variable(MapPatch<VariableDefinition>),
    Page {
        index: usize,
        id: String,
        old: String,
        expected: String,
    },
    Component {
        id: NodeId,
        old: String,
        expected: String,
    },
}
pub(crate) fn supports(command: &C) -> bool {
    mixed::supports(command)
        || upsert::supports(command)
        || variable_rewrite::supports(command)
        || bulk::supports(command)
        || lifecycle::supports(command)
        || plan::supports(command)
}
pub(crate) fn forecast(state: &EditorState, command: &C) -> Option<(usize, bool)> {
    if mixed::supports(command) {
        return mixed::forecast(state, command).map(|bytes| (bytes, false));
    }
    if upsert::supports(command) {
        return upsert::forecast(state, command).map(|bytes| (bytes, false));
    }
    if variable_rewrite::supports(command) {
        return variable_rewrite::forecast(state, command).map(|bytes| (bytes, false));
    }
    if bulk::supports(command) {
        return bulk::forecast(state, command).map(|bytes| (bytes, false));
    }
    if lifecycle::supports(command) {
        return lifecycle::forecast(state, command).map(|bytes| (bytes, false));
    }
    plan::build(state, command).map(|plan| (plan.bytes, plan.capture_components))
}
impl Pending {
    pub(super) fn capture(
        state: &EditorState,
        command: &C,
    ) -> Option<(Self, Vec<Scope>, bool, Vec<super::TargetSpec>)> {
        if mixed::supports(command) {
            let (pending, scopes, targets) = mixed::Pending::capture(state, command)?;
            return Some((
                Self {
                    value: Target::Mixed(pending),
                },
                scopes,
                false,
                targets,
            ));
        }
        if upsert::supports(command) {
            let plan = upsert::Pending::capture(state, command)?;
            return Some((
                Self {
                    value: Target::Upsert(plan.pending),
                },
                plan.scopes,
                false,
                plan.targets,
            ));
        }
        if variable_rewrite::supports(command) {
            let plan = variable_rewrite::Pending::capture(state, command)?;
            return Some((
                Self {
                    value: Target::VariableRewrite(plan.pending),
                },
                plan.scopes,
                false,
                plan.targets,
            ));
        }
        if bulk::supports(command) {
            return Some((
                Self {
                    value: Target::Bulk(bulk::Pending::capture(state, command)?),
                },
                Vec::new(),
                false,
                Vec::new(),
            ));
        }
        if lifecycle::supports(command) {
            let plan = lifecycle::Pending::capture(state, command)?;
            return Some((
                Self {
                    value: Target::Lifecycle(plan.pending),
                },
                plan.scopes,
                false,
                plan.targets,
            ));
        }
        let plan = plan::build(state, command)?;
        Some((
            Self {
                value: plan.value.owned(),
            },
            plan.scopes,
            plan.capture_components,
            Vec::new(),
        ))
    }
    pub(super) fn capture_detach(
        state: &EditorState,
        id: &NodeId,
        path: &[usize],
    ) -> Option<(Self, Vec<Scope>, bool, Vec<super::TargetSpec>)> {
        let plan = lifecycle::Pending::capture_detach(state, id, path)?;
        Some((
            Self {
                value: Target::Lifecycle(plan.pending),
            },
            plan.scopes,
            false,
            plan.targets,
        ))
    }
    pub(super) fn is_header(&self) -> bool {
        matches!(
            self.value,
            Target::Mixed(_)
                | Target::VariableRewrite(_)
                | Target::Bulk(_)
                | Target::Variable { .. }
        )
    }
    pub(super) fn matches_header(&self, doc: &jian_ops_schema::PenDocument) -> bool {
        match &self.value {
            Target::Mixed(value) => value.matches_header(doc),
            Target::Upsert(value) => value.matches_header(doc),
            Target::VariableRewrite(value) => value.matches_header(doc),
            Target::Bulk(value) => value.matches_header(doc),
            Target::Variable {
                key,
                old,
                present,
                total,
                ..
            } => {
                *present == doc.variables.is_some()
                    && *total == doc.variables.as_ref().map_or(0, |map| map.len())
                    && exact(
                        &old.as_ref(),
                        &doc.variables.as_ref().and_then(|map| map.get(key)),
                    )
            }
            _ => true,
        }
    }
    pub(super) fn request_matches(&self, command: &C) -> bool {
        match (&self.value, command) {
            (Target::Mixed(pending), command) => pending.request_matches(command),
            (Target::Upsert(pending), command) => pending.request_matches(command),
            (Target::VariableRewrite(pending), command) => pending.request_matches(command),
            (Target::Bulk(pending), command) => pending.request_matches(command),
            (Target::Lifecycle(pending), command) => pending.request_matches(command),
            (
                Target::Variable {
                    key, create: false, ..
                },
                C::SetVariableColor { name, .. } | C::SetVariableScalar { name, .. },
            ) => key == name,
            (
                Target::Variable {
                    key, create: true, ..
                },
                C::CreateVariable { name, .. },
            ) => key == name.trim(),
            (Target::Page { index, .. }, C::RenamePage { index: at, .. }) => *index == *at as usize,
            (Target::Component { id, .. }, C::RenameComponent { component_id, .. }) => {
                id == component_id
            }
            _ => false,
        }
    }
    pub(super) fn targets_match(&self, state: &EditorState, command: &C) -> bool {
        match &self.value {
            Target::Mixed(pending) => pending.targets_match(state, command),
            _ => true,
        }
    }
    pub(super) fn matches(&self, state: &EditorState) -> bool {
        self.matches_header(&state.doc)
            && match &self.value {
                Target::Mixed(pending) => pending.matches(state),
                Target::Upsert(pending) => pending.matches(state),
                Target::VariableRewrite(pending) => pending.matches(state),
                Target::Bulk(pending) => pending.matches(state),
                Target::Lifecycle(pending) => pending.matches(state),
                Target::Variable { .. } => true,
                Target::Page { index, id, old } => state
                    .doc
                    .pages
                    .as_ref()
                    .and_then(|pages| pages.get(*index))
                    .is_some_and(|page| page.id == *id && page.name == *old),
                Target::Component { index, id, old } => state
                    .components
                    .components
                    .get(*index)
                    .is_some_and(|component| component.id == *id && component.name == *old),
            }
    }
    pub(super) fn compact(self, state: &EditorState) -> Result<Option<Patch>, ScopedHistoryError> {
        Ok(match self.value {
            Target::Mixed(pending) => pending.compact(state)?.map(Patch::Bulk),
            Target::Upsert(_) => return Err(ScopedHistoryError::DocumentHeaderChanged),
            Target::VariableRewrite(pending) => pending.compact(state)?.map(Patch::VariableRewrite),
            Target::Bulk(pending) => pending.compact(state)?.map(Patch::Bulk),
            Target::Lifecycle(pending) => pending.compact(state)?.map(Patch::Catalog),
            Target::Variable {
                key,
                old,
                present,
                total,
                ..
            } => {
                let map = MapPatch::single_key(present, total, key, old, &state.doc.variables);
                if map
                    .as_ref()
                    .is_some_and(|map| !map.validate(&state.doc.variables))
                {
                    return Err(ScopedHistoryError::DocumentHeaderChanged);
                }
                map.map(Patch::Variable)
            }
            Target::Page { index, id, old } => {
                let page = state
                    .doc
                    .pages
                    .as_ref()
                    .and_then(|pages| pages.get(index))
                    .filter(|page| page.id == id)
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
                (page.name != old).then(|| Patch::Page {
                    index,
                    id,
                    old,
                    expected: page.name.clone(),
                })
            }
            Target::Component { index, id, old } => {
                let component = state
                    .components
                    .components
                    .get(index)
                    .filter(|component| component.id == id)
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
                (component.name != old).then(|| Patch::Component {
                    id,
                    old,
                    expected: component.name.clone(),
                })
            }
        })
    }
    pub(super) fn restore(self, state: &mut EditorState) -> Result<(), ScopedHistoryError> {
        if let Some(mut patch) = self.compact(state)? {
            let prepared = Prepared::capture(state, std::slice::from_ref(&patch), false)?;
            prepared.exchange(state, std::slice::from_mut(&mut patch));
        }
        Ok(())
    }
}
impl Patch {
    pub(super) fn owns_ui_refs(&self) -> bool {
        matches!(self, Self::VariableRewrite(_))
    }
    /// Catalog changes are outside PenDocument. Variables/page names are not
    /// described by the canonical node-header visitor and invalidate its delta.
    pub(super) fn changes_document_outside_nodes(&self) -> bool {
        matches!(
            self,
            Self::ConversionUnit(_)
                | Self::VariableRewrite(_)
                | Self::Bulk(_)
                | Self::Variable(_)
                | Self::Page { .. }
        )
    }
    pub(super) fn is_header(&self) -> bool {
        matches!(
            self,
            Self::ConversionUnit(_) | Self::VariableRewrite(_) | Self::Bulk(_) | Self::Variable(_)
        )
    }
    pub(super) fn actor_safe(&self) -> bool {
        match self {
            Self::ConversionUnit(patch) => patch.actor_safe(),
            Self::VariableRewrite(patch) => patch.actor_safe(),
            Self::Bulk(patch) => patch.actor_safe(),
            Self::Variable(map) => map.actor_safe(),
            Self::Catalog(_) => true,
            Self::Page { .. } | Self::Component { .. } => true,
        }
    }
    pub(super) fn actor_conflicts(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::ConversionUnit(a), Self::ConversionUnit(b)) => a.conflicts(b),
            (Self::ConversionUnit(a), Self::Bulk(b)) | (Self::Bulk(b), Self::ConversionUnit(a)) => {
                b.conflicts_unit(a)
            }
            (Self::VariableRewrite(a), Self::VariableRewrite(b)) => a.conflicts(b),
            (Self::VariableRewrite(a), Self::Variable(b))
            | (Self::Variable(b), Self::VariableRewrite(a)) => a.variables().actor_conflicts(b),
            (Self::VariableRewrite(a), Self::Bulk(b))
            | (Self::Bulk(b), Self::VariableRewrite(a)) => b.conflicts_variable(a.variables()),
            (Self::Bulk(a), Self::Bulk(b)) => a.conflicts(b),
            (Self::Bulk(a), Self::Variable(b)) | (Self::Variable(b), Self::Bulk(a)) => {
                a.conflicts_variable(b)
            }
            (Self::Catalog(a), Self::Catalog(b)) => a.id == b.id,
            (Self::Catalog(a), Self::Component { id, .. })
            | (Self::Component { id, .. }, Self::Catalog(a)) => a.id == *id,
            (Self::Variable(a), Self::Variable(b)) => a.actor_conflicts(b),
            (
                Self::Page {
                    index: a, id: x, ..
                },
                Self::Page {
                    index: b, id: y, ..
                },
            ) => {
                let _ = (a, b);
                x == y
            }
            (Self::Component { id: a, .. }, Self::Component { id: b, .. }) => a == b,
            _ => false,
        }
    }
    pub(super) fn retained_roots(
        &self,
    ) -> impl Iterator<Item = &std::sync::Arc<crate::SharedNode>> {
        let entry = match self {
            Self::Catalog(entry) => Some(entry),
            _ => None,
        };
        entry.into_iter().flat_map(catalog::Patch::roots)
    }
    pub(super) fn storage_bytes(&self) -> usize {
        mem::size_of::<Self>()
            + match self {
                Self::ConversionUnit(patch) => patch.bytes(),
                Self::VariableRewrite(patch) => patch.bytes(),
                Self::Bulk(patch) => patch.bytes(),
                Self::Variable(map) => map.storage_bytes(),
                Self::Catalog(patch) => patch.bytes(),
                Self::Page {
                    id, old, expected, ..
                } => id.capacity() + old.capacity() + expected.capacity(),
                Self::Component { id, old, expected } => {
                    id.as_str().len() + old.capacity() + expected.capacity()
                }
            }
    }
}
pub(super) struct Prepared {
    indices: Vec<Option<usize>>,
    catalogs: Vec<Option<catalog::Prepared>>,
}
impl Prepared {
    pub(super) fn capture(
        state: &EditorState,
        patches: &[Patch],
        actor: bool,
    ) -> Result<Self, ScopedHistoryError> {
        let mut indices = Vec::with_capacity(patches.len());
        let mut catalogs = Vec::with_capacity(patches.len());
        for patch in patches {
            if let Patch::Catalog(patch) = patch {
                catalogs.push(Some(catalog::Prepared::capture(state, patch, patches)?));
                indices.push(None);
                continue;
            }
            catalogs.push(None);
            indices.push(match patch {
                Patch::Catalog(_) => unreachable!(),
                Patch::ConversionUnit(patch) => {
                    if !patch.validate(&state.doc, actor) {
                        return Err(ScopedHistoryError::DocumentHeaderChanged);
                    }
                    None
                }
                Patch::VariableRewrite(patch) => {
                    if !patch.validate(state, actor) {
                        return Err(ScopedHistoryError::DocumentHeaderChanged);
                    }
                    None
                }
                Patch::Bulk(patch) => {
                    if !patch.validate(state, actor) {
                        return Err(ScopedHistoryError::DocumentHeaderChanged);
                    }
                    None
                }
                Patch::Variable(map) => {
                    if !(if actor {
                        map.validate_actor(&state.doc.variables)
                    } else {
                        map.validate(&state.doc.variables)
                    }) {
                        return Err(ScopedHistoryError::DocumentHeaderChanged);
                    }
                    None
                }
                Patch::Page {
                    index,
                    id,
                    expected,
                    ..
                } => {
                    let index = super::pages::index_hint(&state.doc, *index, id)
                        .ok_or(ScopedHistoryError::NodePathChanged)?;
                    if state.doc.pages.as_ref().unwrap()[index].name != *expected {
                        return Err(ScopedHistoryError::NodePathChanged);
                    }
                    Some(index)
                }
                Patch::Component { id, expected, .. } => {
                    let mut found = state
                        .components
                        .components
                        .iter()
                        .enumerate()
                        .filter(|(_, component)| component.id == *id);
                    let (index, component) =
                        found.next().ok_or(ScopedHistoryError::NodePathChanged)?;
                    if found.next().is_some() || component.name != *expected {
                        return Err(ScopedHistoryError::NodePathChanged);
                    }
                    Some(index)
                }
            });
        }
        Ok(Self { indices, catalogs })
    }
    pub(super) fn exchange(self, state: &mut EditorState, patches: &mut [Patch]) {
        for ((patch, index), catalog) in patches.iter_mut().zip(self.indices).zip(self.catalogs) {
            match patch {
                Patch::ConversionUnit(patch) => patch.exchange(&mut state.doc),
                Patch::VariableRewrite(patch) => patch.exchange(state),
                Patch::Bulk(patch) => patch.exchange(state),
                Patch::Catalog(patch) => catalog
                    .expect("prepared catalog destination")
                    .exchange(state, patch),
                Patch::Variable(map) => map.exchange(&mut state.doc.variables),
                Patch::Page { old, expected, .. } => {
                    let name = &mut state.doc.pages.as_mut().unwrap()[index.unwrap()].name;
                    mem::swap(name, old);
                    *expected = name.clone();
                }
                Patch::Component { old, expected, .. } => {
                    let at = index.unwrap();
                    state.components.exchange_name_at(at, old);
                    *expected = state.components.components[at].name.clone();
                }
            }
        }
    }
}

impl Patch {
    pub(super) fn cohort_live_bound(&self, state: &EditorState, limit: usize) -> Option<usize> {
        match self {
            Self::ConversionUnit(patch) => patch.live_bound(&state.doc, limit),
            Self::VariableRewrite(patch) => patch.live_bound(state, limit),
            Self::Bulk(patch) => patch.cohort_live_bound(state, limit),
            Self::Catalog(patch) => patch.cohort_live_bound(state, limit),
            Self::Variable(map) => map.cohort_live_bound(&state.doc.variables, limit),
            Self::Page { index, id, .. } => {
                let page = state.doc.pages.as_ref()?.get(*index)?;
                if page.id != *id {
                    return None;
                }
                crate::history_snapshot::command_capture_value_bytes(&page.name, limit).ok()
            }
            Self::Component { id, .. } => {
                let mut matches = state
                    .components
                    .components
                    .iter()
                    .filter(|component| component.id == *id);
                let component = matches.next()?;
                if matches.next().is_some() {
                    return None;
                }
                crate::history_snapshot::command_capture_value_bytes(&component.name, limit).ok()
            }
        }
    }
}

impl Pending {
    pub(super) fn is_upsert(&self) -> bool {
        matches!(self.value, Target::Upsert(_))
    }
    pub(super) fn apply_upsert(&mut self, state: &mut EditorState, command: &C) -> Option<bool> {
        match &mut self.value {
            Target::Upsert(pending) => Some(pending.apply(state, command)),
            _ => None,
        }
    }
    pub(super) fn prepare_page(
        &self,
        state: &EditorState,
        snapshot: &mut super::ScopedEditorSnapshot,
    ) -> Result<(), ScopedHistoryError> {
        if let Target::Upsert(pending) = &self.value {
            pending.prepare_page(state, snapshot)?;
        }
        Ok(())
    }
    pub(super) fn compact_into(
        self,
        state: &EditorState,
        patches: &mut Vec<Patch>,
    ) -> Result<(), ScopedHistoryError> {
        match self.value {
            Target::Upsert(pending) => pending.compact(state, patches),
            value => {
                if let Some(patch) = (Self { value }).compact(state)? {
                    patches.push(patch);
                }
                Ok(())
            }
        }
    }
}
