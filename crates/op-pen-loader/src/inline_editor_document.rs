//! One inline sync/bridge representation, without a document-sized Value clone.
use crate::EditorMeta;
use op_editor_core::EditorState;
use serde::{Serialize, Serializer};

pub struct InlineEditorDocument<'a> {
    document: &'a jian_ops_schema::PenDocument,
    meta: Metadata<'a>,
}
enum Metadata<'a> {
    Captured(&'a EditorMeta),
    Current(&'a EditorState),
}
impl Serialize for Metadata<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Captured(meta) => meta.serialize(serializer),
            // The flattened document serializes first. A capped writer can stop before
            // any catalog/prototype snapshot is allocated for an oversized document.
            Self::Current(state) => EditorMeta::from_state(state).serialize(serializer),
        }
    }
}
impl<'a> InlineEditorDocument<'a> {
    pub fn from_state(state: &'a EditorState) -> Self {
        Self {
            document: &state.doc,
            meta: Metadata::Current(state),
        }
    }
    pub fn with_metadata(document: &'a jian_ops_schema::PenDocument, meta: &'a EditorMeta) -> Self {
        Self {
            document,
            meta: Metadata::Captured(meta),
        }
    }
}
impl Serialize for InlineEditorDocument<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire<'a> {
            #[serde(flatten)]
            document: &'a jian_ops_schema::PenDocument,
            editor_meta: &'a Metadata<'a>,
        }
        // Sync readers consume ordinary inline canonical nodes. Metadata-owned
        // prototypes carry their own independent inner image table.
        jian_ops_schema::image_table::without_save_scope(|| {
            Wire {
                document: self.document,
                editor_meta: &self.meta,
            }
            .serialize(serializer)
        })
    }
}
