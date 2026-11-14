//! Borrowed admission for exact variable keys and page/component labels.
use super::{super::Scope, Target};
use crate::{EditorCommand as C, EditorState};
use jian_ops_schema::variable::VariableDefinition;
const LIMIT: usize = crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE;
pub(super) enum Borrowed<'a> {
    Variable {
        key: &'a str,
        old: Option<&'a VariableDefinition>,
        present: bool,
        total: usize,
        create: bool,
    },
    Page {
        index: usize,
        id: &'a str,
        old: &'a str,
    },
    Component {
        index: usize,
        id: &'a crate::NodeId,
        old: &'a str,
    },
}
pub(super) struct Plan<'a> {
    pub value: Borrowed<'a>,
    pub scopes: Vec<Scope>,
    pub bytes: usize,
    pub capture_components: bool,
}
fn add(bytes: &mut usize, amount: usize) -> Option<()> {
    *bytes = bytes.checked_add(amount)?;
    (*bytes <= LIMIT).then_some(())
}
pub(super) fn supports(command: &C) -> bool {
    matches!(
        command,
        C::SetVariableColor { .. }
            | C::SetVariableScalar { .. }
            | C::CreateVariable { .. }
            | C::RenamePage { .. }
            | C::RenameComponent { .. }
    )
}
pub(super) fn build<'a>(state: &'a EditorState, command: &'a C) -> Option<Plan<'a>> {
    if state.editor_ui.workspace.active
        || state.instance_write_virtual_anchor.is_some()
        || state.undo_capture_suspended
    {
        return None;
    }
    let mut bytes = 64 * 1024usize;
    let mut scopes = Vec::new();
    let value = match command {
        C::SetVariableColor { name, hex } => {
            if crate::color_picker::parse_hex_rgb(hex).is_none() {
                return None;
            }
            let old = state.find_variable(name)?;
            if !matches!(old.kind, jian_ops_schema::variable::VariableKind::Color) {
                return None;
            }
            add(&mut bytes, hex.len().checked_mul(8)?)?;
            variable(state, name, Some(old), false)
        }
        C::SetVariableScalar { name, scalar } => {
            use crate::VariableScalarPayload as V;
            use jian_ops_schema::variable::VariableKind as K;
            let old = state.find_variable(name)?;
            if !matches!(
                (&old.kind, scalar),
                (K::Number, V::Number(_)) | (K::String, V::String(_)) | (K::Boolean, V::Boolean(_))
            ) {
                return None;
            }
            if let V::String(value) = scalar {
                add(&mut bytes, value.len().checked_mul(8)?)?;
            }
            variable(state, name, Some(old), false)
        }
        C::CreateVariable {
            name,
            kind,
            default_value,
        } => {
            let key = name.trim();
            if key.is_empty() || state.find_variable(key).is_some() {
                return None;
            }
            let valid = match kind.as_str() {
                "color" => crate::color_picker::parse_hex_rgb(default_value).is_some(),
                "number" => default_value.trim().parse::<f64>().is_ok(),
                "string" => true,
                "boolean" => matches!(default_value.trim(), "true" | "false"),
                _ => false,
            };
            if !valid {
                return None;
            }
            add(&mut bytes, default_value.len().checked_mul(8)?)?;
            variable(state, key, None, true)
        }
        C::RenamePage { index, name } => {
            if name.trim().is_empty() {
                return None;
            }
            let index = *index as usize;
            let page = state.doc.pages.as_ref()?.get(index)?;
            add(
                &mut bytes,
                (name
                    .len()
                    .checked_add(page.name.len())?
                    .checked_add(page.id.len())?)
                .checked_mul(8)?,
            )?;
            scopes.push(Scope::Page {
                index,
                id: page.id.clone(),
            });
            Borrowed::Page {
                index,
                id: &page.id,
                old: &page.name,
            }
        }
        C::RenameComponent { component_id, name } => {
            if name.trim().is_empty() {
                return None;
            }
            let mut found = state
                .components
                .components
                .iter()
                .enumerate()
                .filter(|(_, component)| &component.id == component_id);
            let (index, component) = found.next()?;
            if found.next().is_some() {
                return None;
            }
            add(
                &mut bytes,
                (name
                    .len()
                    .checked_add(component.name.len())?
                    .checked_add(component.id.as_str().len())?)
                .checked_mul(8)?,
            )?;
            Borrowed::Component {
                index,
                id: &component.id,
                old: &component.name,
            }
        }
        _ => return None,
    };
    if let Borrowed::Variable { key, old, .. } = &value {
        add(&mut bytes, key.len().checked_mul(8)?)?;
        let payload =
            crate::history_snapshot::command_capture_value_bytes(old, LIMIT.checked_sub(bytes)?)
                .ok()?;
        add(&mut bytes, payload)?;
    }
    let capture_components = !matches!(&value, Borrowed::Component { .. })
        && !state
            .components
            .components
            .iter()
            .all(|component| state.components.is_document_backed(&component.id));
    Some(Plan {
        value,
        scopes,
        bytes,
        capture_components,
    })
}
fn variable<'a>(
    state: &EditorState,
    key: &'a str,
    old: Option<&'a VariableDefinition>,
    create: bool,
) -> Borrowed<'a> {
    Borrowed::Variable {
        key,
        old,
        present: state.doc.variables.is_some(),
        total: state.doc.variables.as_ref().map_or(0, |map| map.len()),
        create,
    }
}
impl Borrowed<'_> {
    pub(super) fn owned(&self) -> Target {
        match self {
            Self::Variable {
                key,
                old,
                present,
                total,
                create,
            } => Target::Variable {
                key: (*key).to_owned(),
                old: old.cloned(),
                present: *present,
                total: *total,
                create: *create,
            },
            Self::Page { index, id, old } => Target::Page {
                index: *index,
                id: (*id).to_owned(),
                old: (*old).to_owned(),
            },
            Self::Component { index, id, old } => Target::Component {
                index: *index,
                id: (*id).clone(),
                old: (*old).to_owned(),
            },
        }
    }
}
