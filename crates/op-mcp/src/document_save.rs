//! Document save tool used by the Rust HTTP CLI transport.

use std::collections::BTreeMap;
use std::path::PathBuf;

use op_editor_core::EditorState;
use serde::Serialize;

use super::{McpTool, ToolErrorCode, ToolOutcome};

pub struct SaveDocument {
    document_payload: Result<Vec<u8>, serde_json::Error>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EditorMeta {
    active_page_index: usize,
    preserve_authored_geometry: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    component_catalog: Option<op_editor_core::ComponentCatalog>,
}

impl McpTool for SaveDocument {
    fn name(&self) -> &str {
        "save_document"
    }

    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        let Some(path) = args.get("filePath").filter(|path| !path.trim().is_empty()) else {
            return ToolOutcome::Err(
                ToolErrorCode::MissingArgument,
                "filePath is required".into(),
            );
        };
        let payload = match &self.document_payload {
            Ok(payload) => payload,
            Err(error) => {
                return ToolOutcome::Err(
                    ToolErrorCode::ToolFailed,
                    format!("save document snapshot failed: {error}"),
                )
            }
        };
        let path = resolve_path(path);
        if let Err(e) = std::fs::write(&path, payload) {
            return ToolOutcome::Err(
                ToolErrorCode::ToolFailed,
                format!("save document failed: {e}"),
            );
        }
        let mut out = BTreeMap::new();
        out.insert("ok".into(), "true".into());
        out.insert("filePath".into(), path.display().to_string());
        ToolOutcome::Ok(out)
    }
}

pub fn save_document_snapshot(state: &EditorState) -> SaveDocument {
    // Stream once into the immutable MCP snapshot. The schema writer
    // externalizes shared images while serializing, avoiding both a
    // document-sized `Value` and a second final `String` allocation.
    let mut document_payload = Vec::new();
    let thumbnails = jian_ops_schema::image_thumbs::capture_snapshot();
    let result = jian_ops_schema::image_table::write_document_with_extension(
        &mut document_payload,
        &state.doc,
        &thumbnails,
        "editorMeta",
        &EditorMeta {
            active_page_index: state.ui.active_page_index,
            preserve_authored_geometry: state.editor_ui.preserve_authored_geometry,
            component_catalog: op_editor_core::ComponentCatalog::capture(state),
        },
    );
    SaveDocument {
        document_payload: result.map(|_| document_payload),
    }
}

fn resolve_path(raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}
