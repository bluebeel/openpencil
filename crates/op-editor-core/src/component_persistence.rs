//! Persist independent catalog labels/order and external prototypes without
//! copying document-backed component subtrees into the file or live history.
use super::*;
use serde::{
    de::IntoDeserializer, ser::SerializeMap, Deserialize, Deserializer, Serialize, Serializer,
};
use serde_json::Value;
use std::collections::HashSet;

#[path = "component_persistence_live.rs"]
mod live;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentCatalogError {
    Invalid(String),
    Capture(String),
    PrototypeRoundtrip { id: String },
}
impl std::fmt::Display for ComponentCatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid component catalog metadata: {message}"),
            Self::Capture(message) => write!(f, "component catalog snapshot failed: {message}"),
            Self::PrototypeRoundtrip { id } => write!(
                f,
                "component prototype {id:?} cannot round-trip through canonical JSON"
            ),
        }
    }
}
impl std::error::Error for ComponentCatalogError {}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Row {
    id: String,
    name: String,
    document_backed: bool,
    #[serde(default)]
    root: Option<PenNode>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Wire {
    version: u32,
    components: Vec<Row>,
}
/// Validated immutable editor metadata. Equality covers its persisted JSON;
/// decoded rows are derived and move into the reopened component registry.
#[derive(Debug, Clone)]
pub struct ComponentCatalog {
    value: Arc<Result<Value, ComponentCatalogError>>,
    rows: Arc<Vec<Row>>,
}
impl PartialEq for ComponentCatalog {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl Eq for ComponentCatalog {}
impl Serialize for ComponentCatalog {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.value.as_ref() {
            Ok(value) => value.serialize(serde_stacker::Serializer::new(serializer)),
            Err(error) => Err(serde::ser::Error::custom(error)),
        }
    }
}
impl<'de> Deserialize<'de> for ComponentCatalog {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(serde_stacker::Deserializer::new(deserializer))?;
        let rows = decode(value.clone()).map_err(serde::de::Error::custom)?;
        Ok(Self {
            value: Arc::new(Ok(value)),
            rows: Arc::new(rows),
        })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RowRef<'a> {
    id: &'a str,
    name: &'a str,
    document_backed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    root: Option<&'a PenNode>,
}
struct Body<'a> {
    rows: Vec<RowRef<'a>>,
}
impl Serialize for Body<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // An independent scope/table is essential: outer document writers append
        // editorMeta AFTER their image table, and cannot accept new late refs.
        let thumbnails = jian_ops_schema::image_thumbs::capture_snapshot();
        jian_ops_schema::image_table::with_save_scope(&thumbnails, |tables| {
            let mut map = serializer.serialize_map(None)?;
            map.serialize_entry("version", &1u32)?;
            map.serialize_entry("components", &self.rows)?;
            if !tables.images_is_empty() {
                map.serialize_entry("images", &tables.images())?;
            }
            map.end()
        })
    }
}
fn decode(mut value: Value) -> Result<Vec<Row>, ComponentCatalogError> {
    let table = jian_ops_schema::image_table::take_image_table(&mut value);
    let wire: Wire = jian_ops_schema::node::image_src::intern::with_load_scope(table, || {
        Wire::deserialize(serde_stacker::Deserializer::new(value.into_deserializer()))
    })
    .map_err(|error| ComponentCatalogError::Invalid(error.to_string()))?;
    if wire.version != 1 {
        return Err(ComponentCatalogError::Invalid(format!(
            "unsupported version {}",
            wire.version
        )));
    }
    let mut kinds = HashMap::new();
    for row in &wire.components {
        if !row.document_backed && row.root.is_none() {
            return Err(ComponentCatalogError::Invalid(format!(
                "owned prototype {:?} has no root",
                row.id
            )));
        }
        if kinds
            .insert(row.id.as_str(), row.document_backed)
            .is_some_and(|prior| prior != row.document_backed)
        {
            return Err(ComponentCatalogError::Invalid(format!(
                "conflicting backing for {:?}",
                row.id
            )));
        }
    }
    Ok(wire.components)
}
fn walk<'a>(nodes: &'a [PenNode], visit: &mut impl FnMut(&'a PenNode)) {
    let mut stack = vec![nodes.iter()];
    while let Some(siblings) = stack.last_mut() {
        let Some(node) = siblings.next() else {
            stack.pop();
            continue;
        };
        visit(node);
        if let Some(children) = node.children() {
            if !children.is_empty() {
                stack.push(children.iter());
            }
        }
    }
}
impl ComponentCatalog {
    /// Serialize an exclusively borrowed live registry without constructing and
    /// decoding a second catalog when every prototype has a verified document
    /// location. External or displaced prototypes retain capture validation.
    pub fn borrowed_capture(state: &EditorState) -> Option<impl Serialize + '_> {
        live::capture(state)
    }

    pub fn capture(state: &EditorState) -> Option<Self> {
        Self::capture_library(&state.components, &state.doc)
    }
    fn capture_library(library: &ComponentLibrary, doc: &PenDocument) -> Option<Self> {
        if library.components.is_empty()
            && ComponentLibrary::from_document(doc).components.is_empty()
        {
            return None;
        }
        let missing = library
            .components
            .iter()
            .filter(|component| {
                library.is_document_backed(&component.id)
                    && library
                        .root_at_stored_location(doc, &component.id)
                        .is_none()
            })
            .map(|component| component.id.as_str())
            .collect::<HashSet<_>>();
        let mut found = HashSet::new();
        if !missing.is_empty() {
            let mut visit = |node: &PenNode| {
                if missing.contains(node.id_str()) {
                    found.insert(node.id_str().to_owned());
                }
            };
            walk(&doc.children, &mut visit);
            if let Some(pages) = &doc.pages {
                for page in pages {
                    walk(&page.children, &mut visit);
                }
            }
        }
        let body = Body {
            rows: library
                .components
                .iter()
                .map(|component| {
                    let document_backed = library.is_document_backed(&component.id);
                    let fallback = document_backed
                        && missing.contains(component.id.as_str())
                        && !found.contains(component.id.as_str());
                    RowRef {
                        id: component.id.as_str(),
                        name: &component.name,
                        document_backed,
                        root: (!document_backed || fallback).then_some(&component.root),
                    }
                })
                .collect(),
        };
        let encoded = body
            .serialize(serde_stacker::Serializer::new(
                serde_json::value::Serializer,
            ))
            .map_err(|error| ComponentCatalogError::Capture(error.to_string()));
        let (value, rows) = match encoded {
            Ok(value) => match decode(value.clone()) {
                Ok(rows) => {
                    // Refuse a non-round-trippable owned/fallback prototype before any file
                    // write, including non-finite float values normalized to null by JSON.
                    let invalid = rows.iter().zip(&body.rows).find_map(|(row, source)| {
                        match (&row.root, source.root) {
                            (Some(actual), Some(expected)) if actual != expected => {
                                Some(source.id.to_owned())
                            }
                            _ => None,
                        }
                    });
                    if let Some(id) = invalid {
                        (
                            Err(ComponentCatalogError::PrototypeRoundtrip { id }),
                            Vec::new(),
                        )
                    } else {
                        (Ok(value), rows)
                    }
                }
                Err(error) => (Err(error), Vec::new()),
            },
            Err(error) => (Err(error), Vec::new()),
        };
        Some(Self {
            value: Arc::new(value),
            rows: Arc::new(rows),
        })
    }
    pub fn into_library(
        self,
        doc: &PenDocument,
    ) -> Result<ComponentLibrary, ComponentCatalogError> {
        if let Err(error) = self.value.as_ref() {
            return Err(error.clone());
        }
        let rows = Arc::try_unwrap(self.rows).unwrap_or_else(|rows| rows.as_ref().clone());
        Ok(restore(doc, rows))
    }
    pub fn apply(self, state: &mut EditorState) {
        // Capture errors cannot be serialized; validated input always owns rows.
        if self.value.is_err() {
            return;
        }
        if let Ok(library) = self.into_library(&state.doc) {
            state.components = library;
        }
    }
}
fn restore(doc: &PenDocument, rows: Vec<Row>) -> ComponentLibrary {
    let defaults = ComponentLibrary::from_document(doc);
    let by_id = defaults
        .components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect::<HashMap<_, _>>();
    let wanted = rows
        .iter()
        .filter(|row| row.document_backed)
        .filter(|row| !by_id.contains_key(row.id.as_str()))
        .map(|row| row.id.as_str())
        .collect::<HashSet<_>>();
    let extra = find_roots(doc, &wanted);
    drop(wanted);
    let mut restored = ComponentLibrary::default();
    let mut backing = HashMap::new();
    for row in rows {
        let id = NodeId::new_opt(&row.id).unwrap_or(NodeId::NONE);
        let root = if row.document_backed {
            if let Some(component) = by_id.get(id.as_str()) {
                if let Some(location) = defaults.document_backing.get(&id) {
                    backing.insert(id.clone(), location.clone());
                }
                component.root.clone()
            } else if let Some((node, location)) = extra.get(row.id.as_str()) {
                backing.insert(id.clone(), location.clone());
                shallow_root(node)
            } else if let Some(root) = row.root {
                backing.insert(
                    id.clone(),
                    DocumentNodeLocation {
                        page_index: None,
                        page_id: None,
                        child_path: Box::new([]),
                    },
                );
                root
            } else {
                continue;
            }
        } else {
            row.root.expect("validated owned component root")
        };
        restored.components.push(Component {
            id,
            name: row.name,
            root,
        });
    }
    // When present, the catalog is authoritative. Omitted rows may have been
    // explicitly removed through the public registry API; do not resurrect them.
    restored.document_backing = Arc::new(backing);
    restored
}

// Used only for catalog ids absent from the normal reusable/conversion index.
fn find_roots<'a>(
    doc: &'a PenDocument,
    wanted: &HashSet<&str>,
) -> HashMap<&'a str, (&'a PenNode, DocumentNodeLocation)> {
    fn visit<'a>(
        nodes: &'a [PenNode],
        page: Option<usize>,
        page_id: Option<&Arc<str>>,
        path: &mut Vec<usize>,
        wanted: &HashSet<&str>,
        found: &mut HashMap<&'a str, (&'a PenNode, DocumentNodeLocation)>,
    ) {
        for (index, node) in nodes.iter().enumerate() {
            path.push(index);
            if wanted.contains(node.id_str()) {
                found.entry(node.id_str()).or_insert_with(|| {
                    (
                        node,
                        DocumentNodeLocation {
                            page_index: page,
                            page_id: page_id.cloned(),
                            child_path: path.clone().into_boxed_slice(),
                        },
                    )
                });
            }
            if let Some(children) = node.children() {
                visit(children, page, page_id, path, wanted, found);
            }
            path.pop();
        }
    }
    let mut found = HashMap::new();
    if wanted.is_empty() {
        return found;
    }
    let mut path = Vec::new();
    if let Some(pages) = &doc.pages {
        for (index, page) in pages.iter().enumerate() {
            let id: Arc<str> = Arc::from(page.id.as_str());
            visit(
                &page.children,
                Some(index),
                Some(&id),
                &mut path,
                wanted,
                &mut found,
            );
        }
    }
    visit(&doc.children, None, None, &mut path, wanted, &mut found);
    found
}
impl ComponentLibrary {
    /// Refresh canonical roots for explicitly registered document-backed rows.
    /// Membership/order/labels remain authoritative; discovery belongs to create,
    /// import and page commands. A structural feed must not resurrect a deleted
    /// registration merely because its canonical source is still reusable.
    pub fn rebuild_from_document_preserving_catalog(&mut self, doc: &PenDocument) {
        if self.document_backing.is_empty() {
            return;
        }
        let mut found = HashMap::new();
        let mut wanted = HashSet::new();
        for component in &self.components {
            if let Some(location) = self.document_backing.get(&component.id) {
                if let Some(root) = resolve_document_location(doc, location, component.id.as_str())
                {
                    found.insert(
                        component.id.as_str(),
                        (root, std::borrow::Cow::Borrowed(location)),
                    );
                } else {
                    wanted.insert(component.id.as_str());
                }
            }
        }
        // Every stale route shares one canonical first-identity walk. Unresolved
        // entries retain their existing fallback prototype and registration.
        found.extend(
            find_roots(doc, &wanted)
                .into_iter()
                .map(|(id, (root, location))| (id, (root, std::borrow::Cow::Owned(location)))),
        );
        let mut replacements = Vec::new();
        for (index, component) in self.components.iter().enumerate() {
            if let Some((root, location)) = found.get(component.id.as_str()) {
                let header =
                    (!crate::history_snapshot::SharedNode::metadata_equal_ignoring_children(
                        &component.root,
                        root,
                    ))
                    .then(|| shallow_root(root));
                if header.is_some()
                    || self.document_backing.get(&component.id) != Some(location.as_ref())
                {
                    replacements.push((index, header, location.as_ref().clone()));
                }
            }
        }
        drop(found);
        drop(wanted);
        if replacements.is_empty() {
            return;
        }
        let backing = Arc::make_mut(&mut self.document_backing);
        for (index, header, location) in replacements {
            let component = &mut self.components[index];
            if let Some(header) = header {
                component.root = header;
            }
            backing.insert(component.id.clone(), location);
        }
        // Names/order/identity are unchanged, so the sorted option cache remains
        // valid and owned external prototypes never need cloning.
    }
}

impl ComponentLibrary {
    /// Import names and owned prototypes only for ids not already registered by
    /// the receiving document. Existing identities/labels win on repeat import.
    pub fn merge_imported_catalog(
        &mut self,
        source: Self,
        doc: &PenDocument,
        existing: &HashSet<NodeId>,
    ) {
        let wanted = source
            .components
            .iter()
            .filter(|c| !existing.contains(&c.id) && source.is_document_backed(&c.id))
            .map(|c| c.id.as_str())
            .collect();
        let found = find_roots(doc, &wanted);
        drop(wanted);
        let backing = source.document_backing;
        for component in source.components {
            if existing.contains(&component.id) {
                continue;
            }
            if backing.contains_key(&component.id) {
                if let Some((root, location)) = found.get(component.id.as_str()) {
                    self.insert_document_backed(
                        Component {
                            id: component.id,
                            name: component.name,
                            root: shallow_root(root),
                        },
                        location.clone(),
                    );
                }
            } else {
                self.insert(component);
            }
        }
    }
}
