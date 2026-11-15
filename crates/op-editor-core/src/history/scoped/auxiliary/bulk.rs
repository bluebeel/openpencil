//! Closed bulk header writers: no node forest or component catalog capture.
use super::{exact, EditorState, MapPatch, ScopedHistoryError, C};
use jian_ops_schema::{variable::VariableDefinition, PenDocument};
use std::{collections::BTreeMap, mem};
mod conversion;
mod pins;
mod scalar;
type Variables = BTreeMap<String, VariableDefinition>;
type Themes = BTreeMap<String, Vec<String>>;
const LIMIT: usize = crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE;
#[derive(Debug)]
enum Kind {
    Variables,
    Upsert(String),
    Themes,
    Preset,
    DesignMd,
}
#[derive(Debug)]
pub(super) struct Pending {
    kind: Kind,
    variables: Option<Option<Variables>>,
    themes: Option<Option<Themes>>,
    conversion: Option<conversion::Pending>,
    active_theme: Option<BTreeMap<String, String>>,
    design_md: Option<Option<jian_ops_schema::design_md::DesignMdSpec>>,
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Patch {
    variables: Option<MapPatch<VariableDefinition>>,
    themes: Option<MapPatch<Vec<String>>>,
    conversion: Option<conversion::Patch>,
    pins: Vec<pins::Pin>,
    design_md: Option<scalar::Patch<jian_ops_schema::design_md::DesignMdSpec>>,
}
pub(super) fn supports(command: &C) -> bool {
    matches!(
        command,
        C::SetVariables { .. }
            | C::UpsertVariables { .. }
            | C::SetThemes { .. }
            | C::MergeThemePreset { .. }
            | C::SetDesignMd { .. }
    )
}
fn value_bytes<T: serde::Serialize + ?Sized>(value: &T, bytes: &mut usize) -> Option<()> {
    *bytes = bytes.checked_add(
        crate::history_snapshot::command_capture_value_bytes(&value, LIMIT.checked_sub(*bytes)?)
            .ok()?,
    )?;
    (*bytes <= LIMIT).then_some(())
}
pub(super) fn forecast(state: &EditorState, command: &C) -> Option<usize> {
    if !supports(command)
        || state.editor_ui.workspace.active
        || state.instance_write_virtual_anchor.is_some()
        || state.undo_capture_suspended
    {
        return None;
    }
    let mut bytes = 64 * 1024;
    match command {
        C::SetVariables { variables, .. } => {
            value_bytes(variables, &mut bytes)?;
            value_bytes(&state.doc.variables, &mut bytes)?;
        }
        C::UpsertVariables {
            key,
            variables,
            source_path,
            source_hash,
        } => {
            value_bytes(&(variables, key, source_path, source_hash), &mut bytes)?;
            value_bytes(&state.doc.variables, &mut bytes)?;
            conversion::forecast(&state.doc, key, &mut bytes, LIMIT)?;
        }
        C::SetThemes { themes, .. } => {
            value_bytes(themes, &mut bytes)?;
            value_bytes(&state.doc.themes, &mut bytes)?;
            value_bytes(&state.ui.variables.active_theme, &mut bytes)?;
        }
        C::MergeThemePreset { variables, themes } => {
            value_bytes(&(variables, themes), &mut bytes)?;
            value_bytes(&state.doc.variables, &mut bytes)?;
            value_bytes(&state.doc.themes, &mut bytes)?;
            value_bytes(&state.ui.variables.active_theme, &mut bytes)?;
        }
        C::SetDesignMd { spec } => {
            value_bytes(spec, &mut bytes)?;
            value_bytes(&state.doc.design_md, &mut bytes)?;
        }
        _ => return None,
    }
    Some(bytes)
}
impl Pending {
    pub(super) fn capture(state: &EditorState, command: &C) -> Option<Self> {
        forecast(state, command)?;
        let mut value = Self {
            kind: Kind::Variables,
            variables: None,
            themes: None,
            conversion: None,
            active_theme: None,
            design_md: None,
        };
        match command {
            C::SetVariables { .. } => value.variables = Some(state.doc.variables.clone()),
            C::UpsertVariables { key, .. } => {
                value.kind = Kind::Upsert(key.clone());
                value.variables = Some(state.doc.variables.clone());
                value.conversion = Some(conversion::Pending::capture(&state.doc, key)?);
            }
            C::SetThemes { .. } => {
                value.kind = Kind::Themes;
                value.themes = Some(state.doc.themes.clone());
                value.active_theme = Some(state.ui.variables.active_theme.clone());
            }
            C::MergeThemePreset { .. } => {
                value.kind = Kind::Preset;
                value.variables = Some(state.doc.variables.clone());
                value.themes = Some(state.doc.themes.clone());
                value.active_theme = Some(state.ui.variables.active_theme.clone());
            }
            C::SetDesignMd { .. } => {
                value.kind = Kind::DesignMd;
                value.design_md = Some(state.doc.design_md.clone());
            }
            _ => return None,
        }
        Some(value)
    }
    pub(super) fn request_matches(&self, command: &C) -> bool {
        match (&self.kind, command) {
            (Kind::Variables, C::SetVariables { .. })
            | (Kind::Themes, C::SetThemes { .. })
            | (Kind::Preset, C::MergeThemePreset { .. })
            | (Kind::DesignMd, C::SetDesignMd { .. }) => true,
            (Kind::Upsert(key), C::UpsertVariables { key: next, .. }) => key == next,
            _ => false,
        }
    }
    pub(super) fn matches_header(&self, doc: &PenDocument) -> bool {
        self.design_md
            .as_ref()
            .is_none_or(|old| exact(old, &doc.design_md))
            && self
                .variables
                .as_ref()
                .is_none_or(|old| exact(old, &doc.variables))
            && self
                .themes
                .as_ref()
                .is_none_or(|old| exact(old, &doc.themes))
            && self.conversion.as_ref().is_none_or(|p| p.matches(doc))
    }
    pub(super) fn matches(&self, state: &EditorState) -> bool {
        self.matches_header(&state.doc)
            && self
                .active_theme
                .as_ref()
                .is_none_or(|old| exact(old, &state.ui.variables.active_theme))
    }
    pub(super) fn compact(self, state: &EditorState) -> Result<Option<Patch>, ScopedHistoryError> {
        let design_md = self
            .design_md
            .and_then(|old| scalar::Patch::compact(old, &state.doc.design_md));
        let variables = self
            .variables
            .and_then(|old| MapPatch::compact(old, &state.doc.variables));
        let themes = self
            .themes
            .and_then(|old| MapPatch::compact(old, &state.doc.themes));
        let conversion = self
            .conversion
            .map(|old| old.compact(&state.doc))
            .transpose()?
            .flatten();
        let pins = self.active_theme.map_or_else(Vec::new, |old| {
            pins::compact(old, &state.ui.variables.active_theme)
        });
        if variables.is_none()
            && themes.is_none()
            && conversion.is_none()
            && pins.is_empty()
            && design_md.is_none()
        {
            return Ok(None);
        }
        let patch = Patch {
            variables,
            themes,
            conversion,
            pins,
            design_md,
        };
        if !patch.validate(state, false) {
            return Err(ScopedHistoryError::DocumentHeaderChanged);
        }
        Ok(Some(patch))
    }
}
impl Patch {
    pub(super) fn actor_safe(&self) -> bool {
        self.variables.as_ref().is_none_or(MapPatch::actor_safe)
            && self.themes.as_ref().is_none_or(MapPatch::actor_safe)
            && self
                .conversion
                .as_ref()
                .is_none_or(conversion::Patch::actor_safe)
    }
    pub(super) fn conflicts(&self, other: &Self) -> bool {
        !self.actor_safe()
            || !other.actor_safe()
            || self.design_md.is_some() && other.design_md.is_some()
            || self.variables.as_ref().is_some_and(|a| {
                other
                    .variables
                    .as_ref()
                    .is_some_and(|b| a.actor_conflicts(b))
            })
            || self
                .themes
                .as_ref()
                .is_some_and(|a| other.themes.as_ref().is_some_and(|b| a.actor_conflicts(b)))
            || self
                .conversion
                .as_ref()
                .is_some_and(|a| other.conversion.as_ref().is_some_and(|b| a.conflicts(b)))
    }
    pub(super) fn conflicts_variable(&self, other: &MapPatch<VariableDefinition>) -> bool {
        self.variables
            .as_ref()
            .is_some_and(|a| a.actor_conflicts(other))
    }
    pub(super) fn validate(&self, state: &EditorState, actor: bool) -> bool {
        self.design_md
            .as_ref()
            .is_none_or(|p| p.validate(&state.doc.design_md))
            && self.variables.as_ref().is_none_or(|p| {
                if actor {
                    p.validate_actor(&state.doc.variables)
                } else {
                    p.validate(&state.doc.variables)
                }
            })
            && self.themes.as_ref().is_none_or(|p| {
                if actor {
                    p.validate_actor(&state.doc.themes)
                } else {
                    p.validate(&state.doc.themes)
                }
            })
            && self
                .conversion
                .as_ref()
                .is_none_or(|p| p.validate(&state.doc, actor))
    }
    pub(super) fn exchange(&mut self, state: &mut EditorState) {
        if let Some(p) = &mut self.design_md {
            p.exchange(&mut state.doc.design_md);
        }
        if let Some(p) = &mut self.variables {
            p.exchange(&mut state.doc.variables);
        }
        if let Some(p) = &mut self.themes {
            p.exchange(&mut state.doc.themes);
        }
        if let Some(p) = &mut self.conversion {
            p.exchange(&mut state.doc);
        }
        // Conditional pin receipts never overwrite another actor's later view choice.
        // Canonical themes are exchanged first, so restored pins are also revalidated.
        if self.themes.is_some() || !self.pins.is_empty() {
            pins::exchange(
                &mut self.pins,
                &mut state.ui.variables.active_theme,
                state.doc.themes.as_ref(),
            );
        }
    }
    pub(super) fn bytes(&self) -> usize {
        mem::size_of::<Self>()
            + self.design_md.as_ref().map_or(0, scalar::Patch::bytes)
            + self.variables.as_ref().map_or(0, MapPatch::storage_bytes)
            + self.themes.as_ref().map_or(0, MapPatch::storage_bytes)
            + self.conversion.as_ref().map_or(0, conversion::Patch::bytes)
            + self.pins.capacity() * mem::size_of::<pins::Pin>()
            + self.pins.iter().map(pins::Pin::bytes).sum::<usize>()
    }
    pub(super) fn cohort_live_bound(&self, state: &EditorState, limit: usize) -> Option<usize> {
        let mut bytes = 0usize;
        if self.design_md.is_some() {
            bytes = bytes.checked_add(
                crate::history_snapshot::command_capture_value_bytes(
                    &state.doc.design_md,
                    limit.checked_sub(bytes)?,
                )
                .ok()?,
            )?;
        }
        if let Some(p) = &self.variables {
            bytes = bytes.checked_add(
                p.cohort_live_bound(&state.doc.variables, limit.checked_sub(bytes)?)?,
            )?;
        }
        if let Some(p) = &self.themes {
            bytes = bytes
                .checked_add(p.cohort_live_bound(&state.doc.themes, limit.checked_sub(bytes)?)?)?;
        }
        if let Some(p) = &self.conversion {
            bytes = bytes.checked_add(p.live_bound(&state.doc, limit.checked_sub(bytes)?)?)?;
        }
        bytes = bytes.checked_add(
            crate::history_snapshot::command_capture_value_bytes(
                &state.ui.variables.active_theme,
                limit.checked_sub(bytes)?,
            )
            .ok()?,
        )?;
        (bytes <= limit).then_some(bytes)
    }
}
