//! Live-only catalog encoding. A borrow is never retained across a mutation.
use super::{Component, ComponentCatalog, EditorState};
use serde::{ser::SerializeMap, ser::SerializeSeq, Serialize, Serializer};

pub(super) enum Snapshot<'a> {
    Located(&'a [Component]),
    Captured(ComponentCatalog),
}

pub(super) fn capture(state: &EditorState) -> Option<Snapshot<'_>> {
    let library = &state.components;
    if !library.components.is_empty()
        && library.components.iter().all(|component| {
            library.is_document_backed(&component.id)
                && library
                    .root_at_stored_location(&state.doc, &component.id)
                    .is_some()
        })
    {
        Some(Snapshot::Located(&library.components))
    } else {
        // This also preserves the legacy distinction between an empty catalog
        // and an absent catalog, and all owned-root finite/roundtrip checks.
        ComponentCatalog::capture(state).map(Snapshot::Captured)
    }
}

impl Serialize for Snapshot<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Captured(catalog) => catalog.serialize(serializer),
            Self::Located(components) => {
                // Feature unification can make serde_json::Value retain
                // insertion order. Match the actual capture writer in both
                // dependency configurations, including its exact key order.
                let insertion_order = insertion_order();
                let rows = Rows(components, insertion_order);
                let mut map = serializer.serialize_map(Some(2))?;
                if insertion_order {
                    map.serialize_entry("version", &1u32)?;
                    map.serialize_entry("components", &rows)?;
                } else {
                    map.serialize_entry("components", &rows)?;
                    map.serialize_entry("version", &1u32)?;
                }
                map.end()
            }
        }
    }
}

fn insertion_order() -> bool {
    static ORDER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ORDER.get_or_init(|| {
        let mut map = serde_json::Map::new();
        map.insert("version".into(), serde_json::Value::Null);
        map.insert("components".into(), serde_json::Value::Null);
        map.keys().next().is_some_and(|key| key == "version")
    })
}

struct Rows<'a>(&'a [Component], bool);
impl Serialize for Rows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut rows = serializer.serialize_seq(Some(self.0.len()))?;
        for component in self.0 {
            rows.serialize_element(&Row(component, self.1))?;
        }
        rows.end()
    }
}

struct Row<'a>(&'a Component, bool);
impl Serialize for Row<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut row = serializer.serialize_map(Some(3))?;
        if !self.1 {
            row.serialize_entry("documentBacked", &true)?;
        }
        row.serialize_entry("id", self.0.id.as_str())?;
        row.serialize_entry("name", &self.0.name)?;
        if self.1 {
            row.serialize_entry("documentBacked", &true)?;
        }
        row.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ComponentLibrary, NodeId, PenNodeExt};

    fn state() -> EditorState {
        let doc = jian_ops_schema::load_str(
            r#"{"version":"1.0.0","children":[{"type":"frame","id":"a","name":"A","reusable":true,"children":[]},{"type":"frame","id":"b","name":"B","reusable":true,"children":[]}]}"#,
        )
        .unwrap()
        .value;
        EditorState::from_document(doc)
    }

    fn same_encoding(state: &EditorState) {
        let old = serde_json::to_vec(&ComponentCatalog::capture(state)).unwrap();
        let live = serde_json::to_vec(&ComponentCatalog::borrowed_capture(state)).unwrap();
        assert_eq!(old, live);
    }

    #[test]
    fn live_catalog_matches_captured_bytes_and_borrows_updated_labels() {
        let mut state = state();
        assert!(matches!(capture(&state), Some(Snapshot::Located(_))));
        same_encoding(&state);
        state.components.components.reverse();
        state.components.components[0].name = "quoted \" label \\ \n".into();
        same_encoding(&state);
    }

    #[test]
    fn owned_displaced_and_absent_catalogs_keep_capture_semantics() {
        let mut state = state();
        let mut root = state.doc.children[0].clone();
        root.base_mut().id = "owned".into();
        state.components.insert(Component {
            id: NodeId::new("owned"),
            name: "Owned".into(),
            root,
        });
        assert!(matches!(capture(&state), Some(Snapshot::Captured(_))));
        same_encoding(&state);
        state.components = ComponentLibrary::from_document(&state.doc);
        state.doc.children.clear();
        assert!(matches!(capture(&state), Some(Snapshot::Captured(_))));
        same_encoding(&state);
        state.components = ComponentLibrary::default();
        same_encoding(&state);
    }
}
