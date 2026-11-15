//! UIKit writes only referenced missing variables and one appended clone.
use super::{command_scope, splice};
use crate::{history::TargetSpec, EditorCommand as C, EditorState, PenNodeExt};
const LIMIT: usize = crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE;
pub(super) fn plan(state: &EditorState, command: &C, active: usize) -> Option<TargetSpec> {
    let C::InstantiateKitComponent {
        kit_id,
        component_id,
        target_parent,
        page_id,
        overrides_json,
        ..
    } = command
    else {
        return None;
    };
    // Arbitrary JSON overrides can replace children/type and retain their ordinary path.
    if overrides_json.is_some() {
        return None;
    }
    let kit = state.ui_kits.iter().find(|kit| kit.id == *kit_id)?;
    let component = kit
        .components
        .iter()
        .find(|component| component.id == *component_id)?;
    let page = command_scope::page_index(state, page_id.as_deref(), active)?;
    let scope = command_scope::scope(state, page);
    let mut spec = splice::append_destination(state, &scope, target_parent)?;
    let bytes = bound(state, command)?;
    let edit = spec.splice.as_mut()?;
    edit.forecast = edit.forecast.checked_add(bytes)?;
    // The mutator overwrites only the root name/reusable marker. Descendant
    // geometry, resources and variables stay on the existing implementation.
    let _ = component;
    (edit.forecast <= LIMIT).then_some(spec)
}
pub(super) fn bound(state: &EditorState, command: &C) -> Option<usize> {
    let C::InstantiateKitComponent {
        kit_id,
        component_id,
        overrides_json: None,
        ..
    } = command
    else {
        return None;
    };
    let kit = state.ui_kits.iter().find(|kit| kit.id == *kit_id)?;
    let component = kit
        .components
        .iter()
        .find(|component| component.id == *component_id)?;
    let mut pending = vec![&component.template];
    let mut count = 0usize;
    while let Some(node) = pending.pop() {
        count = count.checked_add(1)?;
        if count > 16_384 {
            return None;
        }
        if let Some(children) = node.children() {
            pending.extend(children);
        }
    }
    // Original authored clone, reminted output, shared inverse, variable ref
    // set and allocation scratch all remain bounded before the first mutation.
    let mut bytes =
        crate::history::structural_payload_bound(&component.template)?.checked_mul(6)?;
    bytes = bytes.checked_add(count.checked_mul(1024)?)?;
    bytes = bytes.checked_add(component.name.len().checked_mul(16)?)?;
    bytes = bytes.checked_add(
        crate::history_snapshot::command_capture_value_bytes(
            &kit.variables,
            LIMIT.checked_sub(bytes)?,
        )
        .ok()?,
    )?;
    (bytes <= LIMIT).then_some(bytes)
}
