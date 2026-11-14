//! Bounded browser pushes preserve the same catalog metadata as disk saves.
use super::{CappedJsonWriter, SyncDocumentJson};
pub(super) fn serialize_sync_state(state: &op_editor_core::EditorState) -> SyncDocumentJson {
    let mut writer = CappedJsonWriter::new();
    let result = serde_json::to_writer(
        &mut writer,
        &op_pen_loader::InlineEditorDocument::from_state(state),
    );
    if result.is_err() {
        return if writer.exceeded {
            SyncDocumentJson::Oversize
        } else {
            SyncDocumentJson::Failed
        };
    }
    match String::from_utf8(writer.bytes) {
        Ok(json) => SyncDocumentJson::Ready(json),
        Err(_) => SyncDocumentJson::Failed,
    }
}
