//! One canonical walk indexes reusable and conversion-declared components.
use super::*;
use jian_ops_schema::conversion::ConversionKind;
use std::collections::HashSet;

struct Collector<'a> {
    wanted: HashSet<&'a str>,
    first: HashMap<&'a str, (&'a PenNode, DocumentNodeLocation)>,
    library: ComponentLibrary,
    positions: HashMap<NodeId, usize>,
}
impl<'a> Collector<'a> {
    fn walk(&mut self, nodes: &'a [PenNode], page_index: Option<usize>, path: &mut Vec<usize>) {
        for (index, node) in nodes.iter().enumerate() {
            path.push(index);
            let id = node.id_str();
            // Keep the first document-order match of any node kind. An earlier
            // non-container duplicate must still prevent a later conversion
            // entry from silently selecting a different component root.
            if !self.wanted.is_empty() && self.wanted.contains(id) && !self.first.contains_key(id) {
                self.first.insert(
                    id,
                    (
                        node,
                        DocumentNodeLocation {
                            page_index,
                            child_path: path.clone().into_boxed_slice(),
                        },
                    ),
                );
            }
            if let PenNode::Frame(frame) = node {
                if frame.reusable == Some(true) {
                    self.library.insert_document_backed_indexed(
                        Component {
                            id: NodeId::new(frame.base.id.clone()),
                            name: frame
                                .base
                                .name
                                .clone()
                                .unwrap_or_else(|| frame.base.id.clone()),
                            root: shallow_root(node),
                        },
                        DocumentNodeLocation {
                            page_index,
                            child_path: path.clone().into_boxed_slice(),
                        },
                        &mut self.positions,
                    );
                }
            }
            if let Some(children) = node.children() {
                self.walk(children, page_index, path);
            }
            path.pop();
        }
    }
}

pub(super) fn from_document(doc: &PenDocument) -> ComponentLibrary {
    let wanted = doc
        .conversion
        .iter()
        .flat_map(|spec| &spec.entries)
        .filter(|entry| entry.kind == ConversionKind::Component)
        .filter_map(|entry| entry.node_id.as_deref().filter(|id| !id.is_empty()))
        .collect();
    let mut collector = Collector {
        wanted,
        first: HashMap::new(),
        library: ComponentLibrary::default(),
        positions: HashMap::new(),
    };
    let mut path = Vec::new();
    if let Some(pages) = &doc.pages {
        for (index, page) in pages.iter().enumerate() {
            collector.walk(&page.children, Some(index), &mut path);
        }
    }
    collector.walk(&doc.children, None, &mut path);
    // Preserve original conversion-entry order: repeated ids replace metadata,
    // and an absent root name falls back to that entry's own caller-stable key.
    for entry in doc.conversion.iter().flat_map(|spec| &spec.entries) {
        if entry.kind != ConversionKind::Component {
            continue;
        }
        let Some(id) = entry.node_id.as_deref().filter(|id| !id.is_empty()) else {
            continue;
        };
        let Some((node, location)) = collector.first.get(id) else {
            continue;
        };
        if !is_component_root(node) {
            continue;
        }
        collector.library.insert_document_backed_indexed(
            Component {
                id: NodeId::new(id),
                name: node
                    .base()
                    .name
                    .clone()
                    .unwrap_or_else(|| entry.key.clone()),
                root: shallow_root(node),
            },
            location.clone(),
            &mut collector.positions,
        );
    }
    collector.library
}
