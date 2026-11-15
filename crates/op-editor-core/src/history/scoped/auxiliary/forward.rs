//! Catalog labels are exact state data, without prototype/document payloads.
use super::*;
use serde::Serialize;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Label<'a> {
    op: &'static str,
    id: &'a str,
    expected_name: &'a str,
    name: &'a str,
}
impl Patch {
    pub(crate) fn forward_label<'a>(&'a self, state: &'a EditorState) -> Option<Label<'a>> {
        let Self::Component { id, old, expected } = self else {
            return None;
        };
        if !id.is_real() || id.as_str().len() > 16 * 1024 {
            return None;
        }
        let mut found = state
            .components
            .components
            .iter()
            .filter(|component| component.id == *id);
        let component = found.next()?;
        if found.next().is_some() || component.name != *expected {
            return None;
        }
        Some(Label {
            op: "set-component-label",
            id: id.as_str(),
            expected_name: old,
            name: &component.name,
        })
    }
}
