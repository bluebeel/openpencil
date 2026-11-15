//! Demand-driven, request-local component/document lookup; no revision trust.
use super::{ComponentLibrary, PenDocument, PenNode};
use crate::{components::Component, pen_node_ext::PenNodeExt};
use std::{cell::RefCell, collections::HashMap, slice};

/// Same depth-first order as the former complete index: all pages, then the
/// legacy root list. Only traversal edges are retained; nodes remain borrowed.
struct DocumentNodes<'a> {
    roots: Vec<&'a [PenNode]>,
    next_root: usize,
    pending: Vec<slice::Iter<'a, PenNode>>,
}
impl<'a> DocumentNodes<'a> {
    fn new(doc: &'a PenDocument) -> Self {
        let mut roots = Vec::with_capacity(doc.pages.as_ref().map_or(1, |p| p.len() + 1));
        if let Some(pages) = &doc.pages {
            roots.extend(pages.iter().map(|p| p.children.as_slice()));
        }
        roots.push(doc.children.as_slice());
        Self {
            roots,
            next_root: 0,
            pending: Vec::new(),
        }
    }
}
impl<'a> Iterator for DocumentNodes<'a> {
    type Item = &'a PenNode;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(iter) = self.pending.last_mut() {
                if let Some(node) = iter.next() {
                    if let Some(children) = node.children() {
                        self.pending.push(children.iter());
                    }
                    return Some(node);
                }
                self.pending.pop();
            } else {
                let roots = *self.roots.get(self.next_root)?;
                self.next_root += 1;
                self.pending.push(roots.iter());
            }
        }
    }
}

/// Single-target callers do not need a catalog map or a retained ID index.
/// Keep the first registered entry's exact stored-route precedence, including
/// document fallback when its route is stale.
pub(super) fn component_target<'a>(
    components: &'a ComponentLibrary,
    document: &'a PenDocument,
    id: &str,
) -> Option<&'a PenNode> {
    if let Some(component) = components
        .components
        .iter()
        .find(|entry| entry.id.as_str() == id)
    {
        let root = if components.is_document_backed(&component.id) {
            components.root_at_stored_location(document, &component.id)
        } else {
            Some(&component.root)
        };
        if root.is_some() {
            return root;
        }
    }
    DocumentNodes::new(document).find(|node| node.id_str() == id)
}

/// A lookup pass advances at most once through the document. Early targets
/// avoid indexing unrelated later pages; late/missing targets still cost only
/// one complete traversal, with first-ID duplicate semantics unchanged.
pub(crate) struct NodeLookup<'a> {
    document: &'a PenDocument,
    components: Option<&'a ComponentLibrary>,
    component_by_id: HashMap<&'a str, &'a Component>,
    component_roots: RefCell<HashMap<&'a str, Option<&'a PenNode>>>,
    document_nodes: RefCell<DocumentNodes<'a>>,
    by_id: RefCell<HashMap<&'a str, &'a PenNode>>,
}
impl<'a> NodeLookup<'a> {
    pub(crate) fn from_document(doc: &'a PenDocument) -> Self {
        Self {
            document: doc,
            components: None,
            component_by_id: HashMap::new(),
            component_roots: RefCell::new(HashMap::new()),
            document_nodes: RefCell::new(DocumentNodes::new(doc)),
            by_id: RefCell::new(HashMap::new()),
        }
    }
    pub(crate) fn from_components_with_document_fallback(
        components: &'a ComponentLibrary,
        document: &'a PenDocument,
    ) -> Self {
        let mut lookup = Self::from_document(document);
        lookup.components = Some(components);
        lookup.component_by_id.reserve(components.len());
        for component in &components.components {
            lookup
                .component_by_id
                .entry(component.id.as_str())
                .or_insert(component);
        }
        lookup
    }
    #[cfg(test)]
    pub(super) fn indexed_document_ids(&self) -> usize {
        self.by_id.borrow().len()
    }

    pub(crate) fn find(&self, id: &str) -> Option<&'a PenNode> {
        // Registry precedence must not be lost when an earlier document scan
        // happened to encounter a different canonical node with the same id.
        if let Some(&component) = self.component_by_id.get(id) {
            let cached = self.component_roots.borrow().get(id).copied();
            let root = match cached {
                Some(root) => root,
                None => {
                    let components = self.components.expect("component registry");
                    let root = if components.is_document_backed(&component.id) {
                        components.root_at_stored_location(self.document, &component.id)
                    } else {
                        Some(&component.root)
                    };
                    self.component_roots
                        .borrow_mut()
                        .insert(component.id.as_str(), root);
                    root
                }
            };
            if root.is_some() {
                return root;
            }
        }
        self.find_authored(id)
    }
    /// Canonical identity lookup must ignore a same-id catalog prototype.
    pub(crate) fn find_authored(&self, id: &str) -> Option<&'a PenNode> {
        if let Some(node) = self.by_id.borrow().get(id).copied() {
            return Some(node);
        }
        let mut nodes = self.document_nodes.borrow_mut();
        let mut by_id = self.by_id.borrow_mut();
        for node in nodes.by_ref() {
            let key = node.id_str();
            let first = *by_id.entry(key).or_insert(node);
            if key == id {
                return Some(first);
            }
        }
        None
    }
}
