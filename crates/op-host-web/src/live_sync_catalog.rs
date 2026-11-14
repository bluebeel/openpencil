//! Bounded browser pushes preserve the same catalog metadata as disk saves.
use super::{CappedJsonWriter, SyncDocumentJson};
use std::io::Write;
pub(super) fn serialize_sync_state(state: &op_editor_core::EditorState) -> SyncDocumentJson {
    let mut writer = CappedJsonWriter::new();
    let result = (|| -> Result<(), serde_json::Error> {
        serde_json::to_writer(&mut writer, &state.doc)?;
        if writer.bytes.pop() != Some(b'}') {
            return Err(serde::ser::Error::custom(
                "canonical document is not an object",
            ));
        }
        writer
            .write_all(b",\"editorMeta\":")
            .map_err(serde_json::Error::io)?;
        serde_json::to_writer(&mut writer, &op_pen_loader::EditorMeta::from_state(state))?;
        writer.write_all(b"}").map_err(serde_json::Error::io)
    })();
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
