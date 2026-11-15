//! Component library — reusable design-system subtrees.
//!
//! Faithful copy of `openpencil-shell-core::document::components`
//! adapted to `op-editor-core`. The crucial difference: a `Component`'s
//! `root` is a canonical `jian_ops_schema::node::PenNode` (the one
//! document model `EditorState` is built on), not shell-core's private
//! `Node`. Component ids stay `op-editor-core::NodeId` string ids so a
//! component is addressable through the same id space as the tree.
//!
use crate::component_backing::{
    find_node_and_location, find_node_in_document, resolve_document_location, shallow_root,
    DocumentComponentBacking, DocumentNodeLocation,
};
use crate::id_allocator::{IdAllocError, IdAllocator};
use crate::node_id::NodeId;
use crate::pen_node_ext::PenNodeExt;
use crate::state::EditorState;
use crate::walkers;
use jian_ops_schema::node::PenNode;
use jian_ops_schema::PenDocument;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

#[path = "component_document_index.rs"]
mod document_index;
#[path = "component_persistence.rs"]
mod persistence;
pub use persistence::{ComponentCatalog, ComponentCatalogError};
#[path = "component_detach.rs"]
mod detach;
#[path = "component_edit.rs"]
mod edit;
#[path = "component_routes.rs"]
mod routes;

/// One reusable design fragment.
///
/// Components inserted directly through [`ComponentLibrary::insert`] own a
/// complete `root`, preserving the original public API. Components indexed
/// from a [`PenDocument`] keep only a children-free compatibility snapshot in
/// this field; use [`ComponentLibrary::resolved_root`] when the complete
/// prototype is required. This avoids retaining a second copy of every
/// reusable document subtree.
#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    pub id: NodeId,
    pub name: String,
    pub root: PenNode,
}

/// Lightweight, name-sorted component metadata for picker surfaces.
///
/// Component roots can be large imported subtrees. Keeping picker rows in a
/// shared slice lets hosts rebuild their immutable panel snapshots without
/// cloning or sorting the full component registry on every frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentOption {
    pub id: String,
    pub name: String,
}

/// Per-document component registry. Populated by the canonical loader;
/// queried by instance-insertion + drag-drop UX.
#[derive(Debug, Clone, Default)]
pub struct ComponentLibrary {
    pub components: Vec<Component>,
    document_backing: DocumentComponentBacking,
    sorted_options: OnceLock<Arc<[ComponentOption]>>,
}

impl PartialEq for ComponentLibrary {
    fn eq(&self, other: &Self) -> bool {
        // The picker cache is derived metadata and must not affect document or
        // history equality merely because one side happened to paint a panel.
        self.components == other.components && self.document_backing == other.document_backing
    }
}

impl ComponentLibrary {
    /// Lexical page-table authority: update routes without a getter Arc clone.
    pub(crate) fn document_backing_mut(&mut self) -> &mut HashMap<NodeId, DocumentNodeLocation> {
        Arc::make_mut(&mut self.document_backing)
    }
    pub(crate) fn apply_page_catalog_delta(
        &mut self,
        remove: &std::collections::HashSet<NodeId>,
        add: Vec<(Component, DocumentNodeLocation)>,
    ) {
        if remove.is_empty() && add.is_empty() {
            return;
        }
        self.invalidate_sorted_options();
        self.components
            .retain(|component| !remove.contains(&component.id));
        {
            let backing = self.document_backing_mut();
            for id in remove {
                backing.remove(id);
            }
            for (component, location) in &add {
                backing.insert(component.id.clone(), location.clone());
            }
        }
        self.components
            .extend(add.into_iter().map(|(component, _)| component));
    }
    /// The caller has validated one exact entry and its rebased row position.
    pub(crate) fn exchange_catalog_entry(
        &mut self,
        index: usize,
        present: bool,
        id: &NodeId,
        replacement: Option<Component>,
        backing: Option<DocumentNodeLocation>,
    ) {
        self.invalidate_sorted_options();
        match (present, replacement) {
            (true, Some(replacement)) => {
                self.components[index] = replacement;
            }
            (true, None) => {
                self.components.remove(index);
            }
            (false, Some(replacement)) => self.components.insert(index, replacement),
            (false, None) => {}
        }
        let locations = Arc::make_mut(&mut self.document_backing);
        if let Some(location) = backing {
            locations.insert(id.clone(), location);
        } else {
            locations.remove(id);
        }
    }
    /// Shared, stable picker metadata sorted by display name then id.
    pub fn sorted_options(&self) -> Arc<[ComponentOption]> {
        Arc::clone(self.sorted_options.get_or_init(|| {
            let mut options: Vec<_> = self
                .components
                .iter()
                .map(|component| ComponentOption {
                    id: component.id.as_str().to_string(),
                    name: component.name.clone(),
                })
                .collect();
            options.sort_by(|left, right| {
                left.name
                    .cmp(&right.name)
                    .then_with(|| left.id.cmp(&right.id))
            });
            Arc::from(options)
        }))
    }

    fn invalidate_sorted_options(&mut self) {
        let _ = self.sorted_options.take();
    }

    pub fn find_by_id(&self, id: &NodeId) -> Option<&Component> {
        self.components.iter().find(|c| &c.id == id)
    }

    pub fn find_by_name(&self, name: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.name == name)
    }

    /// Whether this entry's complete prototype lives in the canonical
    /// document instead of in the registry's compatibility snapshot.
    pub fn is_document_backed(&self, id: &NodeId) -> bool {
        self.document_backing.contains_key(id)
    }

    /// Resolve a component's complete prototype without cloning it.
    ///
    /// Document-backed entries use their compact path first and fall back to
    /// an id walk when an edit has made that path stale. Directly inserted
    /// legacy components continue to borrow their owned `Component::root`.
    pub fn resolved_root<'a>(&'a self, doc: &'a PenDocument, id: &NodeId) -> Option<&'a PenNode> {
        self.resolved_entry(doc, id).map(|(_, root)| root)
    }
    fn resolved_entry<'a>(
        &'a self,
        doc: &'a PenDocument,
        id: &NodeId,
    ) -> Option<(&'a Component, &'a PenNode)> {
        let component = self.find_by_id(id)?;
        let root = match self.document_backing.get(id) {
            None => &component.root,
            Some(location) => resolve_document_location(doc, location, id.as_str())
                .or_else(|| find_node_in_document(doc, id.as_str()))?,
        };
        Some((component, root))
    }

    /// Fast path used by active-page Ref expansion. Unlike
    /// [`resolved_root`](Self::resolved_root), this does not scan the complete
    /// document on a stale path; the resolver owns one shared lazy fallback
    /// index for all misses.
    pub(crate) fn root_at_stored_location<'a>(
        &'a self,
        doc: &'a PenDocument,
        id: &NodeId,
    ) -> Option<&'a PenNode> {
        let location = self.document_backing.get(id)?;
        resolve_document_location(doc, location, id.as_str())
    }

    pub(crate) fn document_backing(&self) -> DocumentComponentBacking {
        Arc::clone(&self.document_backing)
    }

    pub(crate) fn restore_document_backing(&mut self, backing: DocumentComponentBacking) {
        self.document_backing = backing;
    }

    /// Complete roots owned by the legacy/runtime registry rather than by the
    /// canonical document. Document-backed roots must be edited in `doc` and
    /// are deliberately skipped here to avoid double-applying mutations.
    pub(crate) fn owned_roots_mut(&mut self) -> impl Iterator<Item = &mut PenNode> {
        let document_backing = Arc::clone(&self.document_backing);
        self.components
            .iter_mut()
            .filter(move |component| !document_backing.contains_key(&component.id))
            .map(|component| &mut component.root)
    }

    /// Insert a component. Replace-on-duplicate-id mirrors the TS
    /// app's behavior on "Save as Component" of an already-component'd
    /// Frame.
    pub fn insert(&mut self, c: Component) {
        self.invalidate_sorted_options();
        Arc::make_mut(&mut self.document_backing).remove(&c.id);
        if let Some(pos) = self.components.iter().position(|x| x.id == c.id) {
            self.components[pos] = c;
        } else {
            self.components.push(c);
        }
    }

    /// Rename a registered component by id. Returns true when found +
    /// renamed. The empty-string rename is rejected.
    pub fn rename(&mut self, id: &NodeId, new_name: impl Into<String>) -> bool {
        let name: String = new_name.into();
        if name.trim().is_empty() {
            return false;
        }
        let Some(index) = self
            .components
            .iter()
            .position(|component| &component.id == id)
        else {
            return false;
        };
        if self.components[index].name == name {
            return true;
        }
        self.invalidate_sorted_options();
        self.components[index].name = name;
        true
    }
    /// Exact label exchange after all history targets were validated.
    pub(crate) fn exchange_name_at(&mut self, index: usize, name: &mut String) {
        self.invalidate_sorted_options();
        std::mem::swap(&mut self.components[index].name, name);
    }

    /// Remove a component by id. Returns true when a component was
    /// removed. Live instances (clones already on a page) are
    /// unaffected — the registry only holds the prototype.
    pub fn remove(&mut self, id: &NodeId) -> bool {
        if let Some(pos) = self.components.iter().position(|c| &c.id == id) {
            self.invalidate_sorted_options();
            self.components.remove(pos);
            Arc::make_mut(&mut self.document_backing).remove(id);
            true
        } else {
            false
        }
    }

    /// True when no components are registered.
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    /// Count of registered components.
    pub fn len(&self) -> usize {
        self.components.len()
    }

    /// Build a runtime component registry from persisted canonical
    /// `reusable: true` frame nodes. TS treats reusable nodes in the
    /// document tree as component definitions; this keeps loaded `.op`
    /// files addressable even though the registry itself is transient.
    pub fn from_document(doc: &PenDocument) -> Self {
        document_index::from_document(doc)
    }

    fn insert_document_backed(&mut self, component: Component, location: DocumentNodeLocation) {
        self.invalidate_sorted_options();
        Arc::make_mut(&mut self.document_backing).insert(component.id.clone(), location);
        if let Some(pos) = self
            .components
            .iter()
            .position(|existing| existing.id == component.id)
        {
            self.components[pos] = component;
        } else {
            self.components.push(component);
        }
    }

    fn insert_document_backed_indexed(
        &mut self,
        component: Component,
        location: DocumentNodeLocation,
        positions: &mut HashMap<NodeId, usize>,
    ) {
        self.invalidate_sorted_options();
        Arc::make_mut(&mut self.document_backing).insert(component.id.clone(), location);
        if let Some(position) = positions.get(&component.id).copied() {
            self.components[position] = component;
        } else {
            positions.insert(component.id.clone(), self.components.len());
            self.components.push(component);
        }
    }

    /// A prepared canonical placement already owns the exact route. Validate
    /// its final identity instead of walking every unrelated page again.
    pub(crate) fn register_document_component_at(
        &mut self,
        doc: &PenDocument,
        id: NodeId,
        name: String,
        location: DocumentNodeLocation,
    ) -> bool {
        let Some(root) = resolve_document_location(doc, &location, id.as_str()) else {
            return false;
        };
        self.insert_document_backed(
            Component {
                id,
                name,
                root: shallow_root(root),
            },
            location,
        );
        true
    }
    pub(crate) fn register_document_component(
        &mut self,
        doc: &PenDocument,
        id: NodeId,
        name: String,
    ) -> bool {
        let Some((root, location)) = find_node_and_location(doc, id.as_str()) else {
            return false;
        };
        self.insert_document_backed(
            Component {
                id,
                name,
                root: shallow_root(root),
            },
            location,
        );
        true
    }
}

fn is_component_root(node: &PenNode) -> bool {
    matches!(
        node,
        PenNode::Frame(_) | PenNode::Group(_) | PenNode::Rectangle(_)
    )
}

fn set_reusable(node: &mut PenNode, reusable: bool) {
    if let PenNode::Frame(frame) = node {
        frame.reusable = reusable.then_some(true);
    }
}

fn clear_reusable_in_document(doc: &mut jian_ops_schema::PenDocument, id: &NodeId) {
    if let Some(pages) = doc.pages.as_mut() {
        for page in pages {
            if let Some(live) = walkers::find_node_mut(&mut page.children, id) {
                set_reusable(live, false);
                return;
            }
        }
    }
    if let Some(live) = walkers::find_node_mut(&mut doc.children, id) {
        set_reusable(live, false);
    }
}

impl EditorState {
    /// Promote an active-page Frame / Group / Rectangle into the
    /// runtime component registry. Frames also carry the canonical
    /// persisted `reusable` flag so reloads can rebuild the registry.
    pub fn create_component_from_node(&mut self, node_id: &NodeId, name: &str) -> bool {
        edit::create(self, node_id, Some(name))
    }

    /// Promote a node using its current layer name as the component label.
    pub fn create_component_from_node_name(&mut self, node_id: &NodeId) -> bool {
        edit::create(self, node_id, None)
    }

    /// Clone a registered component onto the active page with fresh
    /// ids. The inserted clone is standalone, so any reusable marker
    /// on the prototype root is cleared.
    pub fn instantiate_component(&mut self, component_id: &NodeId) -> Option<NodeId> {
        let mut allocator = crate::command_apply_legacy::LeafHighWaterAllocator(
            self.standalone_id_allocator().ok()?,
        );
        self.instantiate_component_with_allocator(component_id, &mut allocator)
            .ok()
            .flatten()
    }

    /// Allocator-aware form of [`Self::instantiate_component`].
    pub(crate) fn instantiate_component_with_allocator_unobserved(
        &mut self,
        component_id: &NodeId,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        let pending = crate::history::PendingScopedSnapshot::capture_auxiliary(
            self,
            &crate::EditorCommand::InstantiateComponent {
                component_id: component_id.clone(),
            },
        );
        let (mut clone, name) = {
            let Some((component, template)) =
                self.components.resolved_entry(&self.doc, component_id)
            else {
                return Ok(None);
            };
            let mut taken = allocator.document_collision_ids(&self.doc);
            (
                walkers::deep_clone_with_allocator(template, allocator, &mut taken)?,
                component.name.clone(),
            )
        };
        set_reusable(&mut clone, false);
        walkers::translate_subtree(&mut clone, 20.0, 20.0);
        clone.base_mut().name = Some(name);
        let new_id = NodeId::new(clone.base().id.clone());
        let snap = pending
            .is_none()
            .then(|| self.snapshot_for_undo())
            .flatten();
        self.active_children_mut().push(clone);
        self.set_single_selection(new_id.clone());
        self.finish_component_history(pending, snap);
        Ok(Some(new_id))
    }

    /// Point an authored canonical Ref at another registered component.
    /// Direct instance geometry, descendants overrides, bindings, and
    /// events stay on the Ref; only its existing `ref` target changes.
    pub fn set_instance_component(&mut self, node_id: &NodeId, component_id: &NodeId) -> bool {
        if !node_id.is_real()
            || !component_id.is_real()
            || self.components.find_by_id(component_id).is_none()
        {
            return false;
        }
        let Some(path) = routes::find(self.active_children(), node_id.as_str()) else {
            return false;
        };
        let Some(PenNode::Ref(reference)) = routes::at(self.active_children(), &path) else {
            return false;
        };
        if reference.target == component_id.as_str() {
            return false;
        }
        if let Some(changed) = self.try_edit_ref_target_with_history(node_id, component_id.as_str())
        {
            return changed;
        }
        let snapshot = self.snapshot_for_undo();
        let Some(PenNode::Ref(reference)) = routes::at_mut(self.active_children_mut(), &path)
        else {
            return false;
        };
        reference.target.clear();
        reference.target.push_str(component_id.as_str());
        self.push_undo(snapshot);
        true
    }

    /// TS `detachComponent`. A reusable component sheds its flag
    /// (and registry entry); a `Ref` instance materializes into an
    /// independent subtree — `descendants` overrides applied,
    /// instance props overlaid, fresh ids — replacing the Ref in
    /// place. Returns the surviving node's id.
    pub fn detach_component(&mut self, node_id: &NodeId) -> Option<NodeId> {
        detach::standalone(self, node_id)
    }

    /// Allocator-aware form of [`Self::detach_component`].
    pub(crate) fn detach_component_with_allocator_unobserved(
        &mut self,
        node_id: &NodeId,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        let Some(plan) = detach::prepare(self, node_id) else {
            return Ok(None);
        };
        detach::apply(self, plan, allocator)
    }

    /// Remove a component registration. If the source frame is still
    /// on the active page, clear its persisted reusable marker too.
    pub fn delete_component(&mut self, component_id: &NodeId) -> bool {
        if self.components.find_by_id(component_id).is_none() {
            return false;
        }
        let pending = crate::history::PendingScopedSnapshot::capture_auxiliary(
            self,
            &crate::EditorCommand::DeleteComponent {
                component_id: component_id.clone(),
            },
        );
        let snap = pending
            .is_none()
            .then(|| self.snapshot_for_undo())
            .flatten();
        self.components.remove(component_id);
        clear_reusable_in_document(&mut self.doc, component_id);
        self.finish_component_history(pending, snap);
        true
    }

    /// Rename a registered component. This updates the runtime
    /// registry label; the source node's visual/layer name is left
    /// unchanged.
    pub fn rename_component(&mut self, component_id: &NodeId, name: &str) -> bool {
        self.components.rename(component_id, name.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::frame;

    fn sample_node() -> PenNode {
        // A minimal canonical container node fixture.
        frame("f1", "Frame", 0.0, 0.0, 100.0, 100.0, Vec::new())
    }

    fn comp(id: &str, name: &str) -> Component {
        Component {
            id: NodeId::new(id),
            name: name.into(),
            root: sample_node(),
        }
    }

    #[test]
    fn find_by_id_returns_match() {
        let mut lib = ComponentLibrary::default();
        lib.insert(comp("n10", "Button"));
        lib.insert(comp("n11", "Card"));
        assert_eq!(lib.find_by_id(&NodeId::new("n10")).unwrap().name, "Button");
        assert!(lib.find_by_id(&NodeId::new("n99")).is_none());
    }

    #[test]
    fn find_by_name_returns_first_match() {
        let mut lib = ComponentLibrary::default();
        lib.insert(comp("n10", "Button"));
        lib.insert(comp("n11", "Card"));
        assert_eq!(lib.find_by_name("Card").unwrap().id, NodeId::new("n11"));
        assert!(lib.find_by_name("Unknown").is_none());
    }

    #[test]
    fn insert_replaces_duplicate_id() {
        let mut lib = ComponentLibrary::default();
        lib.insert(comp("n10", "Button"));
        lib.insert(comp("n10", "ButtonV2"));
        assert_eq!(lib.len(), 1);
        assert_eq!(
            lib.find_by_id(&NodeId::new("n10")).unwrap().name,
            "ButtonV2"
        );
    }

    #[test]
    fn rename_rejects_empty() {
        let mut lib = ComponentLibrary::default();
        lib.insert(comp("n10", "Button"));
        assert!(!lib.rename(&NodeId::new("n10"), "  "));
        assert!(lib.rename(&NodeId::new("n10"), "Renamed"));
        assert_eq!(lib.find_by_id(&NodeId::new("n10")).unwrap().name, "Renamed");
    }

    #[test]
    fn remove_returns_false_for_unknown() {
        let mut lib = ComponentLibrary::default();
        lib.insert(comp("n10", "Button"));
        assert!(!lib.remove(&NodeId::new("n99")));
        assert!(lib.remove(&NodeId::new("n10")));
        assert!(lib.is_empty());
    }

    #[test]
    fn sorted_options_are_shared_and_invalidated_by_metadata_changes() {
        let mut lib = ComponentLibrary::default();
        lib.insert(comp("n10", "Zulu"));
        lib.insert(comp("n11", "Alpha"));

        let first = lib.sorted_options();
        let second = lib.sorted_options();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first[0].id, "n11");
        assert_eq!(first[1].id, "n10");

        assert!(lib.rename(&NodeId::new("n10"), "Aardvark"));
        let renamed = lib.sorted_options();
        assert!(!Arc::ptr_eq(&first, &renamed));
        assert_eq!(renamed[0].id, "n10");

        lib.insert(comp("n12", "Beta"));
        let inserted = lib.sorted_options();
        assert!(!Arc::ptr_eq(&renamed, &inserted));
        assert_eq!(inserted.len(), 3);
    }

    #[test]
    fn from_document_keeps_only_shallow_metadata_and_resolves_live_root() {
        let mut doc = jian_ops_schema::PenDocument {
            version: "1.0.0".into(),
            name: None,
            themes: None,
            variables: None,
            pages: None,
            children: vec![frame(
                "f1",
                "Frame",
                0.0,
                0.0,
                100.0,
                100.0,
                vec![frame(
                    "nested",
                    "Nested",
                    0.0,
                    0.0,
                    20.0,
                    20.0,
                    vec![sample_node()],
                )],
            )],
            format_version: None,
            id: None,
            app: None,
            routes: None,
            state: None,
            lifecycle: None,
            logic_modules: None,
            design_md: None,
            conversion: None,
            motion: None,
            responsive: None,
        };
        if let PenNode::Frame(f) = &mut doc.children[0] {
            f.reusable = Some(true);
        }
        let lib = ComponentLibrary::from_document(&doc);
        assert_eq!(lib.len(), 1);
        let id = NodeId::new("f1");
        let metadata = lib.find_by_id(&id).unwrap();
        assert_eq!(metadata.name, "Frame");
        assert!(
            metadata.root.children().is_none(),
            "compatibility root must never retain the nested subtree"
        );
        assert!(std::ptr::eq(
            lib.resolved_root(&doc, &id).unwrap(),
            &doc.children[0]
        ));

        // A sibling insert invalidates the compact child-index path. The id
        // verification must reject that path and fall back to the live node.
        doc.children.insert(
            0,
            frame("sibling", "Sibling", 0.0, 0.0, 10.0, 10.0, Vec::new()),
        );
        assert!(std::ptr::eq(
            lib.resolved_root(&doc, &id).unwrap(),
            &doc.children[1]
        ));
        doc.children[1].set_width_px(321.0);
        assert_eq!(
            lib.resolved_root(&doc, &id).unwrap().width_px(),
            Some(321.0)
        );

        doc.children.remove(1);
        assert!(lib.resolved_root(&doc, &id).is_none());
    }
}
