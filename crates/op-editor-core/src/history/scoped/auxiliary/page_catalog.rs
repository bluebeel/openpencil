//! Membership edits for actual added/removed page forests, prepared pre-write.
use super::super::{identity, ScopedEditorSnapshot};
use super::{
    catalog::{self, Entry},
    Patch,
};
use crate::{
    component_backing::DocumentNodeLocation, Component, EditorState, NodeId, PenNodeExt,
    SharedComponent,
};
use jian_ops_schema::{node::PenNode, page::PenPage};
use std::{
    collections::{HashMap, HashSet},
    mem,
    sync::Arc,
};
const LIMIT: usize = crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE;
#[derive(Clone, Copy)]
pub(crate) struct AddedPage<'a> {
    pub page: &'a PenPage,
    pub index_hint: usize,
    pub source: Option<&'a [PenNode]>,
}
pub(crate) struct PreparedPageCatalog {
    remove: HashSet<NodeId>,
    add: Vec<(Component, DocumentNodeLocation)>,
    patches: Vec<(usize, Patch)>,
}
fn source_scope(state: &EditorState, source: &[PenNode]) -> Option<Option<usize>> {
    if source.as_ptr() == state.doc.children.as_ptr() && source.len() == state.doc.children.len() {
        return Some(None);
    }
    for (index, page) in state.doc.pages.iter().flatten().enumerate() {
        if source.as_ptr() == page.children.as_ptr() && source.len() == page.children.len() {
            return Some(Some(index));
        }
    }
    None
}
fn custom_labels<'a>(
    state: &'a EditorState,
    added: AddedPage<'a>,
) -> Option<HashMap<&'a str, &'a str>> {
    let mut labels = HashMap::new();
    let Some(source) = added.source else {
        return Some(labels);
    };
    source_scope(state, source)?;
    let backing = state.components.document_backing();
    let mut registry = HashMap::new();
    for component in &state.components.components {
        if backing.contains_key(&component.id) {
            registry.entry(component.id.as_str()).or_insert(component);
        }
    }
    let lookup = crate::ref_resolve::NodeLookup::from_components_with_document_fallback(
        &state.components,
        &state.doc,
    );
    fn walk<'a>(
        source: &'a [PenNode],
        output: &'a [PenNode],
        registry: &HashMap<&'a str, &'a Component>,
        lookup: &crate::ref_resolve::NodeLookup<'a>,
        labels: &mut HashMap<&'a str, &'a str>,
    ) -> Option<()> {
        if source.len() != output.len() {
            return None;
        }
        for (old, new) in source.iter().zip(output) {
            if mem::discriminant(old) != mem::discriminant(new)
                || old.children().is_some() != new.children().is_some()
            {
                return None;
            }
            if let Some(component) = registry.get(old.id_str()).copied() {
                // Compact routes may be stale after an earlier move. The
                // request-local fallback advances through the Doc only once.
                if lookup
                    .find(old.id_str())
                    .is_some_and(|node| std::ptr::eq(node, old))
                {
                    labels.insert(new.id_str(), component.name.as_str());
                }
            }
            if let Some(children) = old.children() {
                walk(children, new.children()?, registry, lookup, labels)?;
            }
        }
        Some(())
    }
    walk(
        source,
        &added.page.children,
        &registry,
        &lookup,
        &mut labels,
    )?;
    Some(labels)
}
fn trivial(added: Option<AddedPage<'_>>, removed: Option<&PenPage>) -> bool {
    if removed.is_some() {
        return false;
    }
    let Some(added) = added else {
        return true;
    };
    if added.page.children.is_empty() && added.source.is_none_or(|source| source.is_empty()) {
        return true;
    }
    if added.source.is_some() {
        return false;
    }
    let mut reusable = false;
    identity::walk(&added.page.children, |node, _| {
        reusable |= matches!(node,PenNode::Frame(frame) if frame.reusable==Some(true));
    });
    !reusable
}
fn add(total: &mut usize, bytes: usize) -> Option<()> {
    *total = total.checked_add(bytes)?;
    (*total <= LIMIT).then_some(())
}
fn removed_ids(state: &EditorState, removed: Option<&PenPage>) -> Option<HashSet<NodeId>> {
    let mut result = HashSet::new();
    let Some(page) = removed else {
        return Some(result);
    };
    let mut pages = state
        .doc
        .pages
        .as_ref()?
        .iter()
        .filter(|candidate| candidate.id == page.id);
    let actual = pages.next()?;
    if pages.next().is_some() || !std::ptr::eq(actual, page) {
        return None;
    }
    let backing = state.components.document_backing();
    let wanted = state
        .components
        .components
        .iter()
        .filter(|entry| backing.contains_key(&entry.id))
        .map(|entry| entry.id.as_str())
        .collect::<HashSet<_>>();
    if wanted.is_empty() {
        return Some(result);
    }
    let mut selected = HashSet::new();
    identity::walk(&page.children, |node, _| {
        if wanted.contains(node.id_str()) {
            selected.insert(node as *const PenNode);
        }
    });
    if selected.is_empty() {
        return Some(result);
    }
    let lookup = crate::ref_resolve::NodeLookup::from_components_with_document_fallback(
        &state.components,
        &state.doc,
    );
    for component in &state.components.components {
        if !wanted.contains(component.id.as_str()) {
            continue;
        }
        if lookup
            .find(component.id.as_str())
            .is_some_and(|node| selected.contains(&(node as *const PenNode)))
        {
            catalog::position(state, &component.id).ok()??;
            result.insert(component.id.clone());
        }
    }
    Some(result)
}
fn candidates<'a>(
    state: &'a EditorState,
    added: AddedPage<'a>,
    labels: &HashMap<&'a str, &'a str>,
    mut visit: impl FnMut(&'a PenNode, &[usize], &'a str) -> Option<()>,
) -> Option<()> {
    let existing = state
        .components
        .components
        .iter()
        .map(|component| component.id.as_str())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut valid = true;
    identity::walk(&added.page.children, |node, path| {
        if !valid || existing.contains(node.id_str()) {
            return;
        }
        let definition = matches!(node,PenNode::Frame(frame) if frame.reusable==Some(true))
            || labels.contains_key(node.id_str());
        if !definition {
            return;
        }
        if node.id_str().is_empty() || !seen.insert(node.id_str()) {
            valid = false;
            return;
        }
        let label = labels
            .get(node.id_str())
            .copied()
            .or_else(|| node.base().name.as_deref())
            .unwrap_or(node.id_str());
        valid = visit(node, path, label).is_some();
    });
    valid.then_some(())
}
impl PreparedPageCatalog {
    pub(crate) fn forecast(
        state: &EditorState,
        added: Option<AddedPage<'_>>,
        removed: Option<&PenPage>,
    ) -> Option<usize> {
        if trivial(added, removed) {
            return Some(0);
        }
        let remove = removed_ids(state, removed)?;
        let mut bytes = 64usize.checked_mul(1024)?;
        // Arc::make_mut may clone the complete compact route table once. Its
        // paths are independent allocations even though no prototypes are copied.
        let backing = state.components.document_backing();
        add(
            &mut bytes,
            backing
                .capacity()
                .checked_mul(mem::size_of::<(NodeId, DocumentNodeLocation)>().checked_add(32)?)?,
        )?;
        for location in backing.values() {
            add(
                &mut bytes,
                location
                    .child_path
                    .len()
                    .checked_mul(mem::size_of::<usize>())?,
            )?;
        }
        for component in &state.components.components {
            add(
                &mut bytes,
                mem::size_of::<NodeId>() + component.id.as_str().len().checked_mul(4)?,
            )?;
            if remove.contains(&component.id) {
                let bound = crate::history_snapshot::command_capture_node_bytes(
                    &component.root,
                    false,
                    LIMIT.checked_sub(bytes)?,
                )
                .ok()?;
                add(&mut bytes, bound)?;
                add(&mut bytes, component.name.len().checked_mul(4)?)?;
            }
        }
        if let Some(added) = added {
            let labels = custom_labels(state, added)?;
            candidates(state, added, &labels, |node, path, label| {
                let header = crate::history_snapshot::command_capture_node_bytes(
                    node,
                    true,
                    LIMIT.checked_sub(bytes)?,
                )
                .ok()?;
                add(&mut bytes, header.checked_mul(2)?)?;
                add(&mut bytes, label.len().checked_mul(4)?)?;
                add(
                    &mut bytes,
                    path.len()
                        .checked_mul(mem::size_of::<usize>())?
                        .checked_mul(4)?,
                )
            })?;
        }
        Some(bytes)
    }
    pub(crate) fn prepare(
        state: &EditorState,
        added: Option<AddedPage<'_>>,
        removed: Option<&PenPage>,
    ) -> Option<Self> {
        if trivial(added, removed) {
            return Some(Self {
                remove: HashSet::new(),
                add: Vec::new(),
                patches: Vec::new(),
            });
        }
        Self::forecast(state, added, removed)?;
        let remove = removed_ids(state, removed)?;
        let mut patches = Vec::new();
        let mut additions = Vec::new();
        for (index, component) in state.components.components.iter().enumerate() {
            if remove.contains(&component.id) {
                let old = Entry::capture(state, &component.id, None).ok()??;
                patches.push((
                    index,
                    Patch::Catalog(catalog::Patch {
                        id: component.id.clone(),
                        old: Some(old),
                        expected: None,
                        anchor: catalog::anchors(state, &component.id, index),
                        recent: Vec::new(),
                    }),
                ));
            }
        }
        let mut left = state
            .components
            .components
            .iter()
            .rev()
            .find(|component| !remove.contains(&component.id))
            .map(|component| component.id.as_str().to_owned());
        if let Some(added) = added {
            let labels = custom_labels(state, added)?;
            let page_id: Arc<str> = Arc::from(added.page.id.as_str());
            candidates(state, added, &labels, |node, path, label| {
                let id = NodeId::new_opt(node.id_str())?;
                if state.components.document_backing().contains_key(&id) {
                    return None;
                }
                let location = DocumentNodeLocation {
                    page_index: Some(added.index_hint),
                    page_id: Some(Arc::clone(&page_id)),
                    child_path: path.into(),
                };
                let component = Component {
                    id: id.clone(),
                    name: label.to_owned(),
                    root: crate::component_backing::shallow_root(node),
                };
                let expected = Entry {
                    component: SharedComponent::capture(&component, None),
                    backing: Some(location.clone()),
                };
                let rank = state
                    .components
                    .components
                    .len()
                    .checked_add(additions.len())?;
                let anchor = super::super::splice_order::Anchors {
                    ids: vec![id.as_str().to_owned()],
                    left: left.take(),
                    right: None,
                    required: false,
                };
                left = Some(id.as_str().to_owned());
                patches.push((
                    rank,
                    Patch::Catalog(catalog::Patch {
                        id,
                        old: None,
                        expected: Some(expected),
                        anchor,
                        recent: Vec::new(),
                    }),
                ));
                additions.push((component, location));
                Some(())
            })?;
        }
        // Descending original/new row coordinates work for both removal and inverse
        // insertion. The common catalog solver sees every current patch's anchors.
        patches.sort_by_key(|(index, _)| std::cmp::Reverse(*index));
        Some(Self {
            remove,
            add: additions,
            patches,
        })
    }
    /// Ordinary isolated page APIs already own their outer undo/rollback capture.
    /// Membership must be identical to the strict cohort path.
    pub(crate) fn commit_without_history(self, state: &mut EditorState) {
        state
            .components
            .apply_page_catalog_delta(&self.remove, self.add);
    }
    /// The caller has committed exactly the prepared added/removed page payload.
    /// Every membership, prototype and allocation decision was already prepared.
    pub(crate) fn commit(self, state: &mut EditorState, snapshot: &mut ScopedEditorSnapshot) {
        state
            .components
            .apply_page_catalog_delta(&self.remove, self.add);
        snapshot
            .auxiliary_patches
            .extend(self.patches.into_iter().map(|(_, patch)| patch));
    }
}
