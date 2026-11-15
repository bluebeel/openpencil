//! Immediate serializer; no document/catalog snapshot survives its live borrow.
use crate::EditorMeta;
use op_editor_core::{ComponentCatalog, EditorState};
use serde::Serialize;

#[derive(Serialize)]
struct Borrowed<T: Serialize> {
    #[serde(flatten)]
    view: EditorMeta,
    #[serde(rename = "componentCatalog", skip_serializing_if = "Option::is_none")]
    catalog: Option<T>,
}

pub(crate) fn capture(state: &EditorState) -> impl Serialize + '_ {
    Borrowed {
        view: EditorMeta::view_from_state(state),
        catalog: ComponentCatalog::borrowed_capture(state),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_and_snapshot_metadata_have_identical_wire_bytes() {
        let doc = jian_ops_schema::load_str(
            r#"{"version":"1.0.0","children":[{"type":"frame","id":"c","reusable":true,"children":[]}]}"#,
        )
        .unwrap()
        .value;
        let mut state = EditorState::from_document(doc);
        state.editor_ui.preserve_authored_geometry = true;
        state.editor_ui.pinned_style_guide = Some("quoted \" guide".into());
        state.editor_ui.home.imported_from = Some("https://example.com/design".into());
        assert_eq!(
            serde_json::to_vec(&EditorMeta::from_state(&state)).unwrap(),
            serde_json::to_vec(&EditorMeta::borrowed_from_state(&state)).unwrap()
        );
    }
}
