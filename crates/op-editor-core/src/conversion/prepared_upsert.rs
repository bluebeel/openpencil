//! Prepared standalone upserts: one incoming tree, exact canonical destination.
//! Public/custom allocator conversion entrypoints retain their complete path.
use super::*;
use crate::component_backing::DocumentNodeLocation;
use crate::history::Scope;
use std::collections::HashMap;
use std::sync::Arc;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Destination {
    Replace { scope: Scope, path: Box<[usize]> },
    Append { scope: Scope, index: usize },
    NewPage { before: Vec<String>, id: String },
}
#[derive(Debug)]
pub(crate) struct PreparedUpsert {
    pub id: NodeId,
    pub kind: ConversionKind,
    pub key: String,
    pub destination: Destination,
    root: Option<PenNode>,
    name: Option<String>,
    source_path: Option<String>,
    source_hash: Option<String>,
    node_ids: BTreeMap<String, String>,
    next: u64,
}
fn scope(location: &DocumentNodeLocation, state: &EditorState) -> Option<Scope> {
    match location.page_index_in(&state.doc)? {
        Some(index) => Some(Scope::Page {
            index,
            id: state.doc.pages.as_ref()?.get(index)?.id.clone(),
        }),
        None => Some(Scope::Root),
    }
}
fn active(state: &EditorState) -> Scope {
    match state.doc.pages.as_ref().filter(|p| !p.is_empty()) {
        Some(pages) => {
            let index = state.ui.active_page_index.min(pages.len() - 1);
            Scope::Page {
                index,
                id: pages[index].id.clone(),
            }
        }
        None => Scope::Root,
    }
}
fn roots<'a>(state: &'a EditorState, scope: &Scope) -> Option<&'a [PenNode]> {
    match scope {
        Scope::Root => Some(&state.doc.children),
        Scope::Page { index, id } => state
            .doc
            .pages
            .as_ref()?
            .get(*index)
            .filter(|p| p.id == *id)
            .map(|p| p.children.as_slice()),
    }
}
fn unique_entry<'a>(
    state: &'a EditorState,
    kind: ConversionKind,
    key: &str,
) -> Option<Option<&'a ConversionEntry>> {
    let mut entries = state
        .doc
        .conversion
        .iter()
        .flat_map(|s| s.entries.iter())
        .filter(|e| e.kind == kind && e.key == key);
    let first = entries.next();
    entries.next().is_none().then_some(first)
}
fn subtree_ids(node: &PenNode) -> Option<HashSet<String>> {
    let mut ids = HashSet::new();
    let mut stack = vec![(node, 0usize)];
    while let Some((n, depth)) = stack.pop() {
        if depth > 512 || n.id_str().trim().is_empty() || !ids.insert(n.id_str().to_owned()) {
            return None;
        }
        if let Some(c) = n.children() {
            stack.extend(c.iter().map(|c| (c, depth + 1)));
        }
    }
    Some(ids)
}
fn supported_tree(node: &PenNode, root_id: Option<&str>, state: &EditorState) -> bool {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if matches!(n, PenNode::Ref(_)) {
            return false;
        }
        if Some(n.id_str()) != root_id
            && (matches!(n,PenNode::Frame(f)if f.reusable==Some(true))
                || state
                    .components
                    .components
                    .iter()
                    .any(|c| c.id.as_str() == n.id_str()))
        {
            return false;
        }
        if let Some(c) = n.children() {
            stack.extend(c);
        }
    }
    true
}
fn unique_in_document(state: &EditorState, wanted: &HashSet<String>) -> bool {
    let mut counts: HashMap<&str, usize> = wanted.iter().map(|id| (id.as_str(), 0)).collect();
    let mut count = |id: &str| {
        if let Some(n) = counts.get_mut(id) {
            *n += 1;
        }
    };
    let mut stack: Vec<_> = state.doc.children.iter().collect();
    for page in state.doc.pages.iter().flatten() {
        count(&page.id);
        stack.extend(&page.children);
    }
    while let Some(node) = stack.pop() {
        count(node.id_str());
        if let Some(children) = node.children() {
            stack.extend(children);
        }
    }
    counts.values().all(|n| *n == 1)
}
impl PreparedUpsert {
    pub(crate) fn prepare(state: &EditorState, command: &crate::EditorCommand) -> Option<Self> {
        use crate::EditorCommand as C;
        if state.runtime_allocator_depth != 0
            || state.undo_capture_suspended
            || state.editor_ui.workspace.active
            || state.instance_write_virtual_anchor.is_some()
        {
            return None;
        }
        let (kind, key, name, input, source_path, source_hash) = match command {
            C::UpsertComponent {
                key,
                name,
                root,
                source_path,
                source_hash,
            } => {
                if name.trim().is_empty() || !is_component_root(root) {
                    return None;
                }
                (
                    ConversionKind::Component,
                    key.trim(),
                    Some(name.trim().to_owned()),
                    root.as_ref(),
                    source_path.clone(),
                    source_hash.clone(),
                )
            }
            C::UpsertScreen {
                key,
                root,
                source_path,
                source_hash,
            } => {
                if !matches!(root.as_ref(), PenNode::Frame(_)) {
                    return None;
                }
                (
                    ConversionKind::Screen,
                    key.trim(),
                    None,
                    root.as_ref(),
                    source_path.clone(),
                    source_hash.clone(),
                )
            }
            _ => return None,
        };
        if key.is_empty() || !supported_tree(input, Some(input.id_str()), state) {
            return None;
        }
        subtree_ids(input)?;
        if state
            .components
            .components
            .iter()
            .any(|c| !state.components.is_document_backed(&c.id))
        {
            return None;
        }
        let mut pages = HashSet::new();
        if state
            .doc
            .pages
            .iter()
            .flatten()
            .any(|p| p.id.is_empty() || !pages.insert(p.id.as_str()))
        {
            return None;
        }
        let mut catalog = HashSet::new();
        if state
            .components
            .components
            .iter()
            .any(|c| !catalog.insert(c.id.as_str()))
        {
            return None;
        }
        let entry = unique_entry(state, kind, key)?;
        let existing = match entry {
            Some(entry) => {
                let id = entry.node_id.as_deref()?;
                let (node, location) =
                    crate::component_backing::find_node_and_location(&state.doc, id)?;
                Some((node, location))
            }
            None => None,
        };
        let existing_ids = existing_node_ids_for_rerun(input, entry);
        if let Some((node, _)) = &existing {
            let ids = subtree_ids(node)?;
            if !unique_in_document(state, &ids) || !supported_tree(node, Some(node.id_str()), state)
            {
                return None;
            }
            if existing_ids
                .as_ref()
                .is_some_and(|m| m.values().any(|id| !ids.contains(id)))
            {
                return None;
            }
            if kind == ConversionKind::Screen
                && state
                    .components
                    .components
                    .iter()
                    .any(|c| c.id.as_str() == node.id_str())
            {
                return None;
            }
        } else if existing_ids.is_some() {
            return None;
        }
        // Fresh candidates are above the complete live numeric floor. Reused IDs
        // were proven unique and confined to the exact replaced subtree above.
        let mut allocator = crate::command_apply_legacy::LeafHighWaterAllocator(
            state.standalone_id_allocator().ok()?,
        );
        let mut taken = HashSet::new();
        let mut root = input.clone();
        let node_ids = remap_subtree_ids_by_source_id(
            &mut root,
            &mut allocator,
            &mut taken,
            existing_ids.as_ref(),
        )
        .ok()??;
        if let Some(name) = &name {
            root.base_mut().name = Some(name.clone());
        }
        set_reusable(&mut root, kind == ConversionKind::Component);
        let id = NodeId::new(root.id_str());
        if existing
            .as_ref()
            .is_some_and(|(node, _)| node.id_str() != id.as_str())
        {
            return None;
        }
        if state.components.components.iter().any(|c| c.id == id)
            && !state.components.is_document_backed(&id)
        {
            return None;
        }
        if existing.is_none() && state.components.components.iter().any(|c| c.id == id) {
            return None;
        }
        let destination = if let Some((_, location)) = existing {
            Destination::Replace {
                scope: scope(&location, state)?,
                path: location.child_path,
            }
        } else if kind == ConversionKind::Screen {
            let scope = active(state);
            Destination::Append {
                index: roots(state, &scope)?.len(),
                scope,
            }
        } else {
            let pages = state.doc.pages.as_ref().filter(|p| !p.is_empty())?;
            let mut matching = pages
                .iter()
                .enumerate()
                .filter(|(_, p)| p.name == crate::page_mutators::COMPONENTS_PAGE_NAME);
            match matching.next() {
                Some((index, page)) => {
                    if matching.next().is_some() {
                        return None;
                    }
                    Destination::Append {
                        scope: Scope::Page {
                            index,
                            id: page.id.clone(),
                        },
                        index: page.children.len(),
                    }
                }
                None => {
                    let mut seen = HashSet::new();
                    let before: Vec<_> = pages.iter().map(|p| p.id.clone()).collect();
                    if !before.iter().all(|id| seen.insert(id.as_str())) {
                        return None;
                    }
                    let page = allocator.allocate(&mut taken).ok()?;
                    Destination::NewPage {
                        before,
                        id: page.into(),
                    }
                }
            }
        };
        Some(Self {
            id,
            kind,
            key: key.into(),
            destination,
            root: Some(root),
            name,
            source_path,
            source_hash,
            node_ids,
            next: allocator.0.next_counter(),
        })
    }
    pub(crate) fn root(&self) -> &PenNode {
        self.root.as_ref().expect("uncommitted prepared upsert")
    }
    pub(crate) fn matches_destination(&self, state: &EditorState) -> bool {
        match &self.destination {
            Destination::Replace { scope, path } => {
                at(roots(state, scope), path).is_some_and(|n| n.id_str() == self.id.as_str())
            }
            Destination::Append { scope, index } => {
                roots(state, scope).is_some_and(|n| n.len() == *index)
                    && (self.kind != ConversionKind::Screen || *scope == active(state))
            }
            Destination::NewPage { before, .. } => state
                .doc
                .pages
                .as_ref()
                .is_some_and(|p| p.iter().map(|p| &p.id).eq(before)),
        }
    }
    pub(crate) fn commit(&mut self, state: &mut EditorState) -> bool {
        if !self.matches_destination(state) {
            return false;
        }
        let Some(root) = self.root.take() else {
            return false;
        };
        let location = match &self.destination {
            Destination::Replace { scope, path } => {
                let Some(nodes) = roots_mut(state, scope) else {
                    return false;
                };
                let Some(node) = at_mut(nodes, path) else {
                    return false;
                };
                *node = root;
                location(scope, path.clone())
            }
            Destination::Append { scope, index } => {
                let Some(nodes) = roots_mut(state, scope) else {
                    return false;
                };
                nodes.push(root);
                location(scope, vec![*index].into_boxed_slice())
            }
            Destination::NewPage { before, id } => {
                state
                    .doc
                    .pages
                    .as_mut()
                    .expect("prepared paged document")
                    .push(crate::page_mutators::make_page(
                        id.clone(),
                        crate::page_mutators::COMPONENTS_PAGE_NAME.into(),
                        vec![root],
                    ));
                DocumentNodeLocation {
                    page_index: Some(before.len()),
                    page_id: Some(Arc::from(id.as_str())),
                    child_path: Box::new([0]),
                }
            }
        };
        if let Some(name) = &self.name {
            assert!(
                state.components.register_document_component_at(
                    &state.doc,
                    self.id.clone(),
                    name.clone(),
                    location,
                ),
                "validated upsert root lost its canonical destination"
            );
        }
        upsert_conversion_entry(
            &mut state.doc,
            ConversionEntry {
                kind: self.kind,
                key: self.key.clone(),
                source_path: self.source_path.clone(),
                source_hash: self.source_hash.clone(),
                node_id: Some(self.id.as_str().into()),
                node_ids: Some(std::mem::take(&mut self.node_ids)),
            },
        );
        state.record_runtime_next_id(self.next);
        state.mark_document_changed();
        true
    }
}
fn location(scope: &Scope, path: Box<[usize]>) -> DocumentNodeLocation {
    match scope {
        Scope::Root => DocumentNodeLocation {
            page_index: None,
            page_id: None,
            child_path: path,
        },
        Scope::Page { index, id } => DocumentNodeLocation {
            page_index: Some(*index),
            page_id: Some(Arc::from(id.as_str())),
            child_path: path,
        },
    }
}
fn roots_mut<'a>(state: &'a mut EditorState, scope: &Scope) -> Option<&'a mut Vec<PenNode>> {
    match scope {
        Scope::Root => Some(&mut state.doc.children),
        Scope::Page { index, id } => state
            .doc
            .pages
            .as_mut()?
            .get_mut(*index)
            .filter(|p| p.id == *id)
            .map(|p| &mut p.children),
    }
}
fn at<'a>(nodes: Option<&'a [PenNode]>, path: &[usize]) -> Option<&'a PenNode> {
    let mut nodes = nodes?;
    for (depth, &at) in path.iter().enumerate() {
        let node = nodes.get(at)?;
        if depth + 1 == path.len() {
            return Some(node);
        }
        nodes = node.children()?;
    }
    None
}
fn at_mut<'a>(mut nodes: &'a mut [PenNode], path: &[usize]) -> Option<&'a mut PenNode> {
    for (depth, &at) in path.iter().enumerate() {
        let node = nodes.get_mut(at)?;
        if depth + 1 == path.len() {
            return Some(node);
        }
        if node.children().is_none() {
            return None;
        }
        nodes = node.children_mut()?.as_mut_slice();
    }
    None
}
