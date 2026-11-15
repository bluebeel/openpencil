//! Recursive immutable undo snapshots. Canonical node metadata is preserved
//! in full, with child presence and shared edges stored separately. Captures
//! share unchanged descendants and document metadata; COW repair detaches only
//! the changed path. Readers explicitly choose metadata, shared children, or
//! owned materialization. Serialization forwards the canonical schema with a
//! child-value override, retaining exact field order and image traversal.

use crate::component_backing::DocumentComponentBacking;
use crate::components::{Component, ComponentLibrary};
use crate::node_id::NodeId;
use crate::pen_node_ext::PenNodeExt;
use jian_ops_schema::node::PenNode;
use jian_ops_schema::page::PenPage;
use jian_ops_schema::variable::VariableDefinition;
use jian_ops_schema::PenDocument;
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

mod capture_budget;
mod components;
pub use capture_budget::{
    command_capture_node_bytes, command_capture_value_bytes, CaptureSizeError,
    COMMAND_CAPTURE_ALLOWANCE,
};
mod edges;
mod exact;
mod node;
mod node_wire;
mod sensitivity;
pub use components::{SharedComponent, SharedComponents};
use node::share_children;
pub use node::SharedNode;
pub(crate) fn metadata_sensitivity<T: Serialize + ?Sized>(value: &T) -> Box<[u8]> {
    sensitivity::bytes(value)
}
/// Serialize every canonical node metadata field without traversing descendants.
/// An authored `children` field is represented by an empty placeholder only;
/// forward consumers must retain its exact existing value and presence.
pub struct BorrowedNodeMetadata<'a>(pub(crate) &'a PenNode);
impl<'a> BorrowedNodeMetadata<'a> {
    /// Borrow complete schema metadata without cloning or visiting descendants.
    /// The placeholder is unsuitable as a standalone replacement node.
    pub fn new(node: &'a PenNode) -> Self {
        Self(node)
    }
}
impl Serialize for BorrowedNodeMetadata<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(node_wire::ReplaceChildren::new(
            serializer,
            self.0.children().map(|_| SharedNodes(&[][..])),
        ))
    }
}

pub(crate) fn metadata_matches(a: &PenNode, b: &PenNode) -> bool {
    a == b && sensitivity::bytes(a) == sensitivity::bytes(b)
}

/// A structurally-shared mirror of a [`PenDocument`] for undo history.
///
/// The `skeleton` is a full `PenDocument` with every node-children
/// vector emptied (root `children` and each page's `children`); it
/// carries all the cheap scalar / collection fields verbatim. The
/// actual top-level nodes live in `root_children` / `page_children` as
/// shared `Arc`s, parallel to `skeleton.pages`.
#[derive(Debug, Clone, PartialEq)]
pub struct SharedDoc {
    /// The document minus every node-children vector. Cheap to clone
    /// and, being a real `PenDocument`, robust against schema growth.
    skeleton: Arc<PenDocument>,
    metadata_sensitivity: Arc<[u8]>,
    /// Shared top-level nodes of the single-page root `children`.
    root_children: Arc<Vec<Arc<SharedNode>>>,
    /// Shared top-level nodes per page — `page_children[i]` parallels
    /// `skeleton.pages[i]`.
    page_children: Arc<Vec<Arc<Vec<Arc<SharedNode>>>>>,
}

impl SharedDoc {
    /// Capture `doc` into a shared snapshot, reusing `anchor`'s `Arc`s
    /// for any subtree that is unchanged. `anchor` is the
    /// adjacent history state (see [`crate::state::EditorState::snapshot_for_history_with_anchor`]);
    /// `None` produces all-fresh `Arc`s.
    pub fn capture(doc: &PenDocument, anchor: Option<&SharedDoc>) -> SharedDoc {
        let root_children = share_children(&doc.children, anchor.map(|a| &a.root_children));
        let mut page_children = Vec::new();
        if let Some(pages) = doc.pages.as_ref() {
            for (i, page) in pages.iter().enumerate() {
                let prev = anchor.and_then(|a| a.page_children.get(i));
                page_children.push(share_children(&page.children, prev));
            }
        }
        let page_children = edges::reuse(page_children, anchor.map(|a| &a.page_children));
        let metadata = strip_children(doc);
        let signature = sensitivity::bytes(&metadata);
        let reuse = anchor.filter(|old| {
            old.skeleton.as_ref() == &metadata
                && old.metadata_sensitivity.as_ref() == signature.as_ref()
        });
        let skeleton = reuse
            .map(|old| old.skeleton.clone())
            .unwrap_or_else(|| Arc::new(metadata));
        let metadata_sensitivity = reuse
            .map(|old| old.metadata_sensitivity.clone())
            .unwrap_or_else(|| Arc::from(signature));
        SharedDoc {
            skeleton,
            metadata_sensitivity,
            root_children,
            page_children,
        }
    }

    /// Rebuild an owned [`PenDocument`] — materializes every shared node
    /// back into place. The materialized document is byte-identical to
    /// the one that was captured.
    pub fn materialize(&self) -> PenDocument {
        let mut doc = self.skeleton.as_ref().clone();
        doc.children = self.root_children.iter().map(|a| a.materialize()).collect();
        if let Some(pages) = doc.pages.as_mut() {
            for (page, shared) in pages.iter_mut().zip(self.page_children.iter()) {
                page.children = shared.iter().map(|a| a.materialize()).collect();
            }
        }
        doc
    }

    /// The document's variable table, as carried by the skeleton.
    pub fn variables(&self) -> Option<&BTreeMap<String, VariableDefinition>> {
        self.skeleton.variables.as_ref()
    }

    /// Find a node by id within the active page's shared children.
    /// Mirrors the (deliberately un-clamped) snapshot-reader semantics
    /// of the old `snapshot_active_children`: an out-of-range page
    /// index or an empty `pages` list resolves to nothing rather than
    /// falling back to the root `children`.
    pub fn snapshot_find_node(&self, active_page_index: usize, id: &NodeId) -> Option<&SharedNode> {
        let children: &[Arc<SharedNode>] = match self.skeleton.pages.as_ref() {
            Some(pages) => {
                if pages.get(active_page_index).is_some() {
                    self.page_children
                        .get(active_page_index)
                        .map(|v| v.as_slice())
                        .unwrap_or(&[])
                } else {
                    &[]
                }
            }
            None => &self.root_children,
        };
        find_in_shared(children, id)
    }

    /// Copy-on-write repair used by
    /// `instance_override::repair_scope_snapshots`: for every top-level
    /// entry whose subtree holds a *contaminated* (non-`Ref`) node at
    /// `id`, `Arc::make_mut` the entry — isolating it from any sibling
    /// snapshot that shares the `Arc` — then swap the node for
    /// `replacement`. Entries without contamination keep their shared
    /// `Arc` untouched (their `ptr_eq` sharing is preserved).
    pub(crate) fn repair_swap(&mut self, id: &NodeId, replacement: &PenNode) {
        edges::repair(&mut self.root_children, id, replacement);
        if self
            .page_children
            .iter()
            .any(|p| edges::needs_repair(p, id))
        {
            for page in Arc::make_mut(&mut self.page_children) {
                edges::repair(page, id, replacement);
            }
        }
    }

    /// Shared top-level nodes of the single-page root. Test / assertion
    /// hook for `ptr_eq` sharing checks.
    pub fn root_children(&self) -> &[Arc<SharedNode>] {
        &self.root_children
    }
    /// Immutable edge owner for weak-safe snapshot cache identities.
    pub fn root_children_arc(&self) -> &Arc<Vec<Arc<SharedNode>>> {
        &self.root_children
    }
    /// Immutable page-edge table; each entry independently owns its node edges.
    pub fn page_children_arcs(&self) -> &Arc<Vec<Arc<Vec<Arc<SharedNode>>>>> {
        &self.page_children
    }

    /// Borrow snapshot metadata without materializing its shared node trees.
    pub fn document_metadata(&self) -> &PenDocument {
        &self.skeleton
    }
    /// Immutable metadata identity for deduplicated resident accounting.
    pub fn metadata_arc(&self) -> &Arc<PenDocument> {
        &self.skeleton
    }

    /// Allocation identities and complete retained vector storage, for a
    /// resident ledger to deduplicate edge arrays shared across checkpoints.
    pub fn edge_storage(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        std::iter::once(edges::storage(&self.root_children))
            .chain(std::iter::once(edges::storage(&self.page_children)))
            .chain(self.page_children.iter().map(edges::storage))
    }

    pub fn metadata_storage_bytes(&self) -> usize {
        std::mem::size_of::<PenDocument>()
            + 4 * std::mem::size_of::<usize>()
            + self.metadata_sensitivity.len()
            + self
                .skeleton
                .pages
                .as_ref()
                .map_or(0, |p| p.capacity() * std::mem::size_of::<PenPage>())
    }

    /// Shared top-level nodes of page `i`, if any.
    pub fn page_children(&self, i: usize) -> Option<&[Arc<SharedNode>]> {
        self.page_children.get(i).map(|v| v.as_slice())
    }
}

impl jian_ops_schema::image_table::SaveImageOrder for SharedDoc {
    fn visit_save_image_sources(&self, visit: &mut dyn FnMut(&jian_ops_schema::node::ImageSrc)) {
        fn visit_roots(
            nodes: &[Arc<SharedNode>],
            visit: &mut dyn FnMut(&jian_ops_schema::node::ImageSrc),
        ) {
            let mut stack: Vec<_> = nodes.iter().map(Arc::as_ref).collect();
            while let Some(node) = stack.pop() {
                jian_ops_schema::image_table::visit_legacy_node_roots(
                    std::iter::once(node.metadata()),
                    visit,
                );
                if let Some(children) = node.children() {
                    stack.extend(children.iter().map(Arc::as_ref));
                }
            }
        }
        visit_roots(&self.root_children, visit);
        for page in self.page_children.iter().rev() {
            visit_roots(page, visit)
        }
    }
}

/// Serialize the shared snapshot directly as the canonical `PenDocument`
/// wire shape. This lets background saves keep unchanged subtrees
/// behind `Arc` instead of materializing a second owned document first.
impl Serialize for SharedDoc {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let doc = &self.skeleton;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("version", &doc.version)?;
        serialize_option(&mut map, "name", &doc.name)?;
        serialize_option(&mut map, "themes", &doc.themes)?;
        serialize_option(&mut map, "variables", &doc.variables)?;
        if doc.pages.is_some() {
            map.serialize_entry("pages", &SharedPages(self))?;
        }
        map.serialize_entry("children", &SharedNodes(&self.root_children))?;
        serialize_option(&mut map, "formatVersion", &doc.format_version)?;
        serialize_option(&mut map, "responsive", &doc.responsive)?;
        serialize_option(&mut map, "id", &doc.id)?;
        serialize_option(&mut map, "app", &doc.app)?;
        serialize_option(&mut map, "routes", &doc.routes)?;
        serialize_option(&mut map, "state", &doc.state)?;
        serialize_option(&mut map, "lifecycle", &doc.lifecycle)?;
        serialize_option(&mut map, "logicModules", &doc.logic_modules)?;
        serialize_option(&mut map, "designMd", &doc.design_md)?;
        serialize_option(&mut map, "conversion", &doc.conversion)?;
        serialize_option(&mut map, "motion", &doc.motion)?;
        map.end()
    }
}

fn serialize_option<M, T>(map: &mut M, key: &'static str, value: &Option<T>) -> Result<(), M::Error>
where
    M: SerializeMap,
    T: Serialize,
{
    if let Some(value) = value {
        map.serialize_entry(key, value)?;
    }
    Ok(())
}

struct SharedNodes<'a>(&'a [Arc<SharedNode>]);

impl Serialize for SharedNodes<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for node in self.0 {
            seq.serialize_element(node.as_ref())?;
        }
        seq.end()
    }
}

struct SharedPages<'a>(&'a SharedDoc);

impl Serialize for SharedPages<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let pages = self.0.skeleton.pages.as_deref().unwrap_or_default();
        let mut seq = serializer.serialize_seq(Some(pages.len()))?;
        for (index, page) in pages.iter().enumerate() {
            let children = self
                .0
                .page_children
                .get(index)
                .map(|v| v.as_slice())
                .unwrap_or_default();
            seq.serialize_element(&SharedPage { page, children })?;
        }
        seq.end()
    }
}

struct SharedPage<'a> {
    page: &'a PenPage,
    children: &'a [Arc<SharedNode>],
}

impl Serialize for SharedPage<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("id", &self.page.id)?;
        map.serialize_entry("name", &self.page.name)?;
        map.serialize_entry("children", &SharedNodes(self.children))?;
        serialize_option(&mut map, "backgroundColor", &self.page.background_color)?;
        serialize_option(&mut map, "state", &self.page.state)?;
        serialize_option(&mut map, "lifecycle", &self.page.lifecycle)?;
        map.end()
    }
}

// --- Free helpers ----------------------------------------------------

/// A `PenDocument` with every node-children vector emptied — the
/// snapshot skeleton. Written as an explicit struct literal (no
/// `..Default`) so a new `PenDocument` field fails to compile here
/// until it is classified, keeping the skeleton schema-complete.
pub(crate) fn strip_children(doc: &PenDocument) -> PenDocument {
    PenDocument {
        version: doc.version.clone(),
        name: doc.name.clone(),
        themes: doc.themes.clone(),
        variables: doc.variables.clone(),
        pages: doc
            .pages
            .as_ref()
            .map(|pages| pages.iter().map(strip_page_children).collect()),
        children: Vec::new(),
        format_version: doc.format_version.clone(),
        id: doc.id.clone(),
        app: doc.app.clone(),
        routes: doc.routes.clone(),
        state: doc.state.clone(),
        lifecycle: doc.lifecycle.clone(),
        logic_modules: doc.logic_modules.clone(),
        design_md: doc.design_md.clone(),
        conversion: doc.conversion.clone(),
        motion: doc.motion,
        // The responsive schema opt-in is document-level metadata, not
        // node content — survives the children-strip like every other
        // metadata field above.
        responsive: doc.responsive,
    }
}

/// A `PenPage` with its `children` emptied. Explicit literal, same
/// schema-completeness rationale as [`strip_children`].
fn strip_page_children(page: &PenPage) -> PenPage {
    PenPage {
        id: page.id.clone(),
        name: page.name.clone(),
        background_color: page.background_color.clone(),
        state: page.state.clone(),
        lifecycle: page.lifecycle.clone(),
        children: Vec::new(),
    }
}

/// Locate a recursive shared view without materializing container descendants.
fn find_in_shared<'a>(children: &'a [Arc<SharedNode>], id: &NodeId) -> Option<&'a SharedNode> {
    children.iter().find_map(|node| node.find_node(id))
}

#[cfg(test)]
#[path = "history_snapshot/recursive_tests.rs"]
mod recursive_tests;

#[cfg(test)]
#[path = "history_snapshot/edge_tests.rs"]
mod edge_tests;

#[cfg(test)]
mod exact_tests;
