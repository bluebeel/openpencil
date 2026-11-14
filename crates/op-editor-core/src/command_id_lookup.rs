//! Node locations live only inside a locked, observed command authority.
//! Parent tokens stay stable when a sibling list moves. No public document or
//! revision is trusted outside that authority and clones never inherit it.
use crate::{history::Scope, EditorCommand as C, EditorState, NodeId, PenNodeExt};
use jian_ops_schema::{node::PenNode, PenDocument};
use std::{cmp::Ordering, collections::HashMap, sync::Arc};
#[derive(Debug, Clone, Copy)]
pub(crate) enum Order {
    Preorder,
    SiblingFirst,
    LastPreorder,
    LastNonEmptyPostorder,
}
#[derive(Debug)]
struct Entry {
    parent: Option<usize>,
    slot: usize,
    root: usize,
    id: Arc<str>,
    children: Vec<usize>,
}
#[derive(Debug)]
enum Locations {
    One(usize),
    Many(Vec<usize>),
}
impl Locations {
    fn add(&mut self, token: usize) {
        match self {
            Self::One(old) => *self = Self::Many(vec![*old, token]),
            Self::Many(keys) => keys.push(token),
        }
    }
    fn keys(&self) -> &[usize] {
        match self {
            Self::One(key) => std::slice::from_ref(key),
            Self::Many(keys) => keys,
        }
    }
    fn remove(&mut self, token: usize) -> bool {
        match self {
            Self::One(key) => *key == token,
            Self::Many(keys) => {
                keys.retain(|key| *key != token);
                if keys.len() == 1 {
                    *self = Self::One(keys[0]);
                }
                self.keys().is_empty()
            }
        }
    }
}
#[derive(Debug)]
pub(super) struct Index {
    entries: Vec<Entry>,
    ids: HashMap<Arc<str>, Locations>,
    pages: Vec<(String, usize)>,
}
impl Index {
    fn new(doc: &PenDocument) -> Self {
        let mut index = Self {
            entries: Vec::new(),
            ids: HashMap::new(),
            pages: Vec::new(),
        };
        let root = index.add_root();
        index.append(root, &doc.children);
        for page in doc.pages.as_deref().unwrap_or_default() {
            let root = index.add_root();
            index.pages.push((page.id.clone(), root));
            index.append(root, &page.children);
        }
        index
    }
    /// Forecast only index-owned ID/location allocations before cloning any
    /// ID text. Conservative capacity/reallocation headroom covers Vec/HashMap
    /// growth; oversized read scopes simply keep their fresh legacy traversal.
    pub(super) fn try_new(doc: &PenDocument, limit: usize) -> Option<Self> {
        let row = std::mem::size_of::<Entry>()
            .checked_mul(4)?
            .checked_add((std::mem::size_of::<(Arc<str>, Locations)>() + 32).checked_mul(8)?)?
            .checked_add(4 * std::mem::size_of::<usize>() + 64)?;
        let mut bytes = 64usize.checked_mul(1024)?;
        let mut pending = Vec::new();
        pending.push(doc.children.as_slice());
        bytes = bytes.checked_add(row)?;
        for page in doc.pages.as_deref().unwrap_or_default() {
            bytes = bytes
                .checked_add(row)?
                .checked_add(page.id.len())?
                .checked_add(4 * std::mem::size_of::<(String, usize)>())?;
            if bytes > limit {
                return None;
            }
            pending.push(page.children.as_slice());
        }
        while let Some(nodes) = pending.pop() {
            for node in nodes {
                bytes = bytes.checked_add(row)?.checked_add(node.id_str().len())?;
                if bytes > limit {
                    return None;
                }
                if let Some(children) = node.children() {
                    pending.push(children);
                }
            }
        }
        if bytes > limit {
            return None;
        }
        let index = Self::new(doc);
        (index.storage_bytes() <= limit).then_some(index)
    }
    fn add_root(&mut self) -> usize {
        let key = self.entries.len();
        self.entries.push(Entry {
            parent: None,
            slot: 0,
            root: key,
            id: Arc::from(""),
            children: Vec::new(),
        });
        key
    }
    fn append(&mut self, parent: usize, nodes: &[PenNode]) {
        let mut stack = vec![(parent, nodes.iter())];
        while let Some((parent, siblings)) = stack.last_mut() {
            let Some(node) = siblings.next() else {
                stack.pop();
                continue;
            };
            let parent = *parent;
            let key = self.entries.len();
            let id: Arc<str> = self
                .ids
                .get_key_value(node.id_str())
                .map_or_else(|| Arc::from(node.id_str()), |(id, _)| Arc::clone(id));
            match self.ids.entry(Arc::clone(&id)) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(Locations::One(key));
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => entry.get_mut().add(key),
            }
            self.entries.push(Entry {
                parent: Some(parent),
                slot: self.entries[parent].children.len(),
                root: self.entries[parent].root,
                id,
                children: Vec::new(),
            });
            self.entries[parent].children.push(key);
            if let Some(children) = node.children().filter(|children| !children.is_empty()) {
                stack.push((key, children.iter()));
            }
        }
    }
    pub(super) fn storage_bytes(&self) -> usize {
        let mut bytes = std::mem::size_of::<Self>()
            .saturating_add(
                self.entries
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Entry>()),
            )
            .saturating_add(
                self.ids
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(Arc<str>, Locations)>() + 32),
            )
            .saturating_add(
                self.pages
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(String, usize)>()),
            );
        for entry in &self.entries {
            if entry.parent.is_none() {
                bytes = bytes.saturating_add(2 * std::mem::size_of::<usize>());
            }
            bytes = bytes.saturating_add(
                entry
                    .children
                    .capacity()
                    .saturating_mul(std::mem::size_of::<usize>()),
            );
        }
        for (id, locations) in &self.ids {
            bytes = bytes.saturating_add(id.len() + 2 * std::mem::size_of::<usize>());
            if let Locations::Many(keys) = locations {
                bytes = bytes
                    .saturating_add(keys.capacity().saturating_mul(std::mem::size_of::<usize>()));
            }
        }
        for (id, _) in &self.pages {
            bytes = bytes.saturating_add(id.capacity());
        }
        bytes
    }
    fn root(&self, scope: &Scope) -> Option<usize> {
        match scope {
            Scope::Root => Some(0),
            Scope::Page { index, id } => self
                .pages
                .get(*index)
                .filter(|(page, _)| page == id)
                .map(|(_, root)| *root),
        }
    }
    fn path(&self, mut key: usize) -> Box<[usize]> {
        let mut path = Vec::new();
        while let Some(parent) = self.entries[key].parent {
            path.push(self.entries[key].slot);
            key = parent;
        }
        path.reverse();
        path.into_boxed_slice()
    }
    fn select(&self, root: usize, id: &str, order: Order) -> Option<usize> {
        let keys = self.ids.get(id)?.keys();
        if let [key] = keys {
            let entry = &self.entries[*key];
            return (entry.root == root
                && (!matches!(order, Order::LastNonEmptyPostorder) || !entry.children.is_empty()))
            .then_some(*key);
        }
        let mut best: Option<(usize, Box<[usize]>)> = None;
        for &key in keys {
            if self.entries[key].root != root
                || matches!(order, Order::LastNonEmptyPostorder)
                    && self.entries[key].children.is_empty()
            {
                continue;
            }
            let path = self.path(key);
            let replace = best.as_ref().is_none_or(|(_, old)| match order {
                Order::Preorder => path.as_ref() < old.as_ref(),
                Order::SiblingFirst => sibling_priority(&path, old).is_lt(),
                Order::LastPreorder => path.as_ref() > old.as_ref(),
                Order::LastNonEmptyPostorder => postorder_priority(&path, old).is_gt(),
            });
            if replace {
                best = Some((key, path));
            }
        }
        best.map(|(key, _)| key)
    }
    fn query(&self, scope: &Scope, id: &str, order: Order) -> Option<Option<Box<[usize]>>> {
        let root = self.root(scope)?;
        Some(self.select(root, id, order).map(|key| self.path(key)))
    }
    fn remove(&mut self, parent: usize, at: usize) {
        let key = self.entries[parent].children.remove(at);
        for slot in at..self.entries[parent].children.len() {
            let key = self.entries[parent].children[slot];
            self.entries[key].slot = slot;
        }
        let mut pending = vec![key];
        while let Some(key) = pending.pop() {
            let row = &mut self.entries[key];
            if self
                .ids
                .get_mut(&row.id)
                .is_some_and(|keys| keys.remove(key))
            {
                self.ids.remove(&row.id);
            }
            pending.append(&mut row.children);
            row.parent = None;
            row.id = Arc::from("");
        }
    }
}
fn postorder_priority(a: &[usize], b: &[usize]) -> Ordering {
    if a.starts_with(b) || b.starts_with(a) {
        b.len().cmp(&a.len())
    } else {
        a.cmp(b)
    }
}
fn sibling_priority(a: &[usize], b: &[usize]) -> Ordering {
    let common = a.iter().zip(b).take_while(|(a, b)| a == b).count();
    if common == a.len() || common == b.len() {
        return a.len().cmp(&b.len());
    }
    match (a.len() == common + 1, b.len() == common + 1) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a[common].cmp(&b[common]),
    }
}
#[derive(Debug)]
enum Edit {
    Append,
    Remove(usize),
}
#[derive(Debug)]
pub(super) struct Pending {
    scope: Scope,
    parent: usize,
    len: usize,
    revision: u64,
    command: Box<C>,
    edit: Edit,
}
impl Pending {
    fn prepare(index: &Index, state: &EditorState, command: &C) -> Option<Self> {
        let (raw, id, append) = match command {
            C::CopyNode {
                page_id,
                target_parent,
                ..
            } => (page_id.as_deref(), target_parent, true),
            C::DeleteNode { page_id, node_id } => (page_id.as_deref(), node_id, false),
            _ => return None,
        };
        let scope = state.command_lookup_scope(raw, state.ui.active_page_index)?;
        let root = index.root(&scope)?;
        let (parent, edit) = if append {
            (
                if id.is_real() {
                    index.select(root, id.as_str(), Order::SiblingFirst)?
                } else {
                    root
                },
                Edit::Append,
            )
        } else {
            let key = index.select(root, id.as_str(), Order::SiblingFirst)?;
            (
                index.entries[key].parent?,
                Edit::Remove(index.entries[key].slot),
            )
        };
        Some(Self {
            scope,
            parent,
            len: index.entries[parent].children.len(),
            revision: state.document_revision(),
            command: Box::new(command.clone()),
            edit,
        })
    }
    fn before(&self, index: &Index, state: &EditorState) -> bool {
        state.document_revision() == self.revision
            && children_at(&state.doc, &self.scope, &index.path(self.parent))
                .is_some_and(|nodes| nodes.len() == self.len)
    }
    fn finish(&self, index: &mut Index, state: &EditorState) -> Option<()> {
        if self.before(index, state) {
            return Some(());
        }
        let nodes = children_at(&state.doc, &self.scope, &index.path(self.parent))?;
        match self.edit {
            Edit::Append if nodes.len() == self.len.checked_add(1)? => {
                if self.len > 0 {
                    let old = index.entries[self.parent].children[self.len - 1];
                    if nodes[self.len - 1].id_str() != index.entries[old].id.as_ref() {
                        return None;
                    }
                }
                index.append(self.parent, std::slice::from_ref(&nodes[self.len]));
            }
            Edit::Remove(at) if nodes.len().checked_add(1)? == self.len => {
                if at < nodes.len() {
                    let next = *index.entries[self.parent].children.get(at + 1)?;
                    if nodes[at].id_str() != index.entries[next].id.as_ref() {
                        return None;
                    }
                }
                index.remove(self.parent, at);
            }
            _ => return None,
        }
        Some(())
    }
}
#[derive(Debug, Default)]
pub(super) enum Cache {
    #[default]
    Unbuilt,
    Bounded(usize),
    Ready(Index),
    Pending(Index, Pending),
    Invalid,
}
impl Cache {
    pub(super) fn new(index: Option<Index>) -> Self {
        index.map_or(Self::Unbuilt, Self::Ready)
    }
    pub(super) fn bounded(index: Option<Index>, limit: usize) -> Self {
        index.map_or(Self::Bounded(limit), Self::Ready)
    }
    pub(super) fn ready(&mut self, state: &EditorState) -> bool {
        if let Self::Bounded(limit) = self {
            *self = Index::try_new(&state.doc, *limit).map_or(Self::Invalid, Self::Ready);
        } else if matches!(self, Self::Unbuilt) {
            *self = Self::Ready(Index::new(&state.doc));
        }
        match self {
            Self::Ready(_) => true,
            Self::Pending(index, pending) => pending.before(index, state),
            _ => false,
        }
    }
    pub(super) fn path(
        &mut self,
        state: &EditorState,
        scope: &Scope,
        id: &str,
        order: Order,
    ) -> Option<Option<Box<[usize]>>> {
        if !self.ready(state) {
            return None;
        }
        match self {
            Self::Ready(index) | Self::Pending(index, _) => index.query(scope, id, order),
            _ => None,
        }
    }
    pub(super) fn observe(&mut self, state: &EditorState, command: &C) {
        if preserves_locations(command) {
            return;
        }
        let prior = std::mem::replace(self, Self::Invalid);
        match prior {
            Self::Ready(index) => {
                if let Some(pending) = Pending::prepare(&index, state, command) {
                    *self = Self::Pending(index, pending);
                }
            }
            Self::Pending(index, pending)
                if pending.command.as_ref() == command && pending.before(&index, state) =>
            {
                // Scoped apply and raw allocator apply both observe before the
                // same command. No second canonical mutation occurred here.
                *self = Self::Pending(index, pending);
            }
            _ => {}
        }
    }
    pub(super) fn finish(self, state: &EditorState) -> Option<Index> {
        match self {
            Self::Ready(index) => Some(index),
            Self::Pending(mut index, pending) => {
                pending.finish(&mut index, state)?;
                Some(index)
            }
            _ => None,
        }
    }
}
// Numeric identity preservation is weaker than location preservation: Move,
// ReorderSelected and ReorderPage keep ids but relocate nodes. Only this
// explicit header/view set may reuse routes; future commands start uncached.
fn preserves_locations(command: &C) -> bool {
    let mut pending = vec![command];
    let mut count = 0usize;
    while let Some(command) = pending.pop() {
        count += 1;
        if count > 4096 {
            return false;
        }
        if let C::Batch { commands } = command {
            pending.extend(commands);
            continue;
        }
        if !command.may_change_document() {
            continue;
        }
        if matches!(command, C::PatchNodeData { .. } if crate::command_node_attrs::bulk::prepare(command).is_some())
        {
            continue;
        }
        if !matches!(
            command,
            C::UpdateNode { .. }
                | C::SetNodeRotation { .. }
                | C::SetNodeText { .. }
                | C::SetNodeCornerRadius { .. }
                | C::SetNodeFontSize { .. }
                | C::SetNodeFontWeight { .. }
                | C::SetNodeStrokeHex { .. }
                | C::SetNodeStrokeWidth { .. }
                | C::SetNodeStrokeSideWidth { .. }
                | C::SetNodeFillHex { .. }
                | C::SetNodeName { .. }
                | C::SetNodeFlag { .. }
                | C::SetNodeFlip { .. }
                | C::SetEllipseArc { .. }
                | C::AddNodeEffect { .. }
                | C::RemoveNodeEffect { .. }
                | C::SetEffectParam { .. }
                | C::SetEffectColor { .. }
                | C::SetNodeLayoutProp { .. }
                | C::NudgeSelected { .. }
                | C::AlignSelected { .. }
                | C::ReplaceFontFamily { .. }
                | C::SetVariableColor { .. }
                | C::SetVariableScalar { .. }
                | C::CreateVariable { .. }
                | C::DeleteVariable { .. }
                | C::RenameVariable { .. }
                | C::SetVariables { .. }
                | C::UpsertVariables { .. }
                | C::SetThemes { .. }
                | C::MergeThemePreset { .. }
                | C::MergeAppState { .. }
                | C::SetDesignMd { .. }
                | C::RenamePage { .. }
                | C::CreateComponent { .. }
                | C::DeleteComponent { .. }
                | C::RenameComponent { .. }
        ) {
            return false;
        }
    }
    true
}

fn roots<'a>(doc: &'a PenDocument, scope: &Scope) -> Option<&'a [PenNode]> {
    match scope {
        Scope::Root => Some(&doc.children),
        Scope::Page { index, id } => doc
            .pages
            .as_ref()?
            .get(*index)
            .filter(|page| page.id == *id)
            .map(|page| page.children.as_slice()),
    }
}
fn children_at<'a>(doc: &'a PenDocument, scope: &Scope, path: &[usize]) -> Option<&'a [PenNode]> {
    let mut nodes = roots(doc, scope)?;
    for &at in path {
        nodes = nodes.get(at)?.children().map_or(&[], Vec::as_slice);
    }
    Some(nodes)
}
fn at<'a>(doc: &'a PenDocument, scope: &Scope, path: &[usize]) -> Option<&'a PenNode> {
    let (last, parents) = path.split_last()?;
    children_at(doc, scope, parents)?.get(*last)
}
impl super::SeedOverride {
    pub(super) fn observe_lookup(&self, state: &EditorState, command: &C) {
        if self.entered && self.valid && self.generation == state.document_generation() {
            if let Ok(mut cache) = self.lookup.lock() {
                cache.observe(state, command);
            }
        }
    }
    pub(super) fn take_lookup(&mut self, state: &EditorState) -> Option<Index> {
        std::mem::take(&mut self.lookup)
            .into_inner()
            .ok()?
            .finish(state)
    }
}
impl EditorState {
    pub(crate) fn command_lookup_path(
        &self,
        scope: &Scope,
        id: &NodeId,
        order: Order,
    ) -> Option<Option<Box<[usize]>>> {
        let seed = &self.command_id_seed;
        if !seed.entered || !seed.valid || seed.generation != self.document_generation() {
            return None;
        }
        seed.lookup
            .lock()
            .ok()?
            .path(self, scope, id.as_str(), order)
    }
    pub(crate) fn command_find_node(&self, scope: &Scope, id: &NodeId) -> Option<&PenNode> {
        match self.command_lookup_path(scope, id, Order::Preorder) {
            Some(path) => path.and_then(|path| at(&self.doc, scope, &path)),
            None => crate::walkers::find_node(roots(&self.doc, scope)?, id),
        }
    }
    #[allow(clippy::result_large_err)]
    pub(crate) fn command_append_at(
        &mut self,
        scope: &Scope,
        path: &[usize],
        node: PenNode,
    ) -> Result<(), PenNode> {
        let Some(nodes) = list_mut(&mut self.doc, scope, path, true) else {
            return Err(node);
        };
        nodes.push(node);
        Ok(())
    }
    pub(crate) fn command_remove_at(&mut self, scope: &Scope, path: &[usize]) -> Option<PenNode> {
        let (at, parents) = path.split_last()?;
        let nodes = list_mut(&mut self.doc, scope, parents, false)?;
        (*at < nodes.len()).then(|| nodes.remove(*at))
    }
}
fn list_mut<'a>(
    doc: &'a mut PenDocument,
    scope: &Scope,
    path: &[usize],
    author: bool,
) -> Option<&'a mut Vec<PenNode>> {
    let mut nodes = match scope {
        Scope::Root => &mut doc.children,
        Scope::Page { index, id } => {
            let page = doc.pages.as_mut()?.get_mut(*index)?;
            if page.id != *id {
                return None;
            }
            &mut page.children
        }
    };
    for (depth, &at) in path.iter().enumerate() {
        let node = nodes.get_mut(at)?;
        if node.children().is_none() && (!author || depth + 1 < path.len()) {
            return None;
        }
        nodes = node.children_mut()?;
    }
    Some(nodes)
}
