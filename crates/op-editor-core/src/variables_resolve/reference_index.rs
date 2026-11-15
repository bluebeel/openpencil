//! Bounded exact token census. It contains metadata routes, never node copies.
use crate::{history::Scope, PenNodeExt};
use jian_ops_schema::{node::PenNode, PenDocument};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_TARGETS: usize = 4096;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Route {
    pub scope: Scope,
    pub path: Box<[usize]>,
    pub id: String,
}
#[derive(Debug)]
pub(crate) struct Index {
    by_token: HashMap<String, Vec<Arc<Route>>>,
    overfull: HashSet<String>,
    ambiguous: HashSet<String>,
    duplicate_pages: HashSet<String>,
}
#[derive(Debug, Default)]
pub(crate) enum Cache {
    #[default]
    Empty,
    Ready(Index),
    Unavailable,
}
impl Cache {
    pub(crate) fn new(index: Option<Index>) -> Self {
        index.map_or(Self::Empty, Self::Ready)
    }
    pub(crate) fn routes(&mut self, doc: &PenDocument, name: &str) -> Option<Vec<Arc<Route>>> {
        if matches!(self, Self::Empty) {
            *self = Index::build(doc).map_or(Self::Unavailable, Self::Ready);
        }
        if let Self::Ready(index) = self {
            index.routes(name)
        } else {
            None
        }
    }
    pub(crate) fn finish(self) -> Option<Index> {
        if let Self::Ready(index) = self {
            Some(index)
        } else {
            None
        }
    }
    pub(crate) fn refresh(&mut self, doc: &PenDocument, routes: &[Arc<Route>]) {
        if let Self::Ready(index) = self {
            if index.refresh(doc, routes).is_none() {
                *self = Self::Empty;
            }
        }
    }
}
pub(crate) fn walk(
    nodes: &[PenNode],
    mut visit: impl FnMut(&PenNode, &[usize]) -> Option<()>,
) -> Option<()> {
    let mut path = Vec::new();
    let mut stack = vec![(nodes.iter().enumerate(), 0usize)];
    while let Some((siblings, depth)) = stack.last_mut() {
        let Some((at, node)) = siblings.next() else {
            stack.pop();
            continue;
        };
        let depth = *depth;
        if depth > 512 {
            return None;
        }
        path.truncate(depth);
        path.push(at);
        visit(node, &path)?;
        if let Some(children) = node.children().filter(|c| !c.is_empty()) {
            let depth = path.len();
            stack.push((children.iter().enumerate(), depth));
        }
    }
    Some(())
}
fn scopes(doc: &PenDocument, mut visit: impl FnMut(Scope, &[PenNode]) -> Option<()>) -> Option<()> {
    visit(Scope::Root, &doc.children)?;
    for (index, page) in doc.pages.as_deref().unwrap_or_default().iter().enumerate() {
        visit(
            Scope::Page {
                index,
                id: page.id.clone(),
            },
            &page.children,
        )?;
    }
    Some(())
}
impl Index {
    pub(crate) fn build(doc: &PenDocument) -> Option<Self> {
        let mut index = Self {
            by_token: HashMap::new(),
            overfull: HashSet::new(),
            ambiguous: HashSet::new(),
            duplicate_pages: HashSet::new(),
        };
        let mut charged = 64 * 1024usize;
        let mut pages = HashSet::new();
        for page in doc.pages.as_deref().unwrap_or_default() {
            charged = charged.checked_add(page.id.len().checked_add(256)?)?;
            if charged > MAX_BYTES {
                return None;
            }
            if !pages.insert(page.id.as_str()) {
                index.duplicate_pages.insert(page.id.clone());
            }
        }
        scopes(doc, |scope, nodes| {
            walk(nodes, |node, path| {
                let mut names = HashSet::new();
                super::visit_reference_fields(node, &mut |name| {
                    names.insert(name);
                });
                let mut route = None;
                for name in names {
                    if index.overfull.contains(name) {
                        continue;
                    }
                    if index
                        .by_token
                        .get(name)
                        .is_some_and(|v| v.len() >= MAX_TARGETS)
                    {
                        index.by_token.remove(name);
                        index.overfull.insert(name.into());
                        continue;
                    }
                    charged = charged.checked_add(
                        4 * (std::mem::size_of::<Arc<Route>>()
                            + std::mem::size_of::<Route>()
                            + 128
                            + name.len()
                            + node.id_str().len()
                            + match &scope {
                                Scope::Root => 0,
                                Scope::Page { id, .. } => id.len(),
                            }
                            + path.len() * std::mem::size_of::<usize>()),
                    )?;
                    if charged > MAX_BYTES {
                        return None;
                    }
                    let route = route.get_or_insert_with(|| {
                        Arc::new(Route {
                            scope: scope.clone(),
                            path: path.into(),
                            id: node.id_str().into(),
                        })
                    });
                    index
                        .by_token
                        .entry(name.into())
                        .or_default()
                        .push(route.clone());
                }
                Some(())
            })
        })?;
        // Detect only relevant duplicate IDs. No full-document ID set is retained.
        let mut counts: HashMap<&str, usize> = index
            .by_token
            .values()
            .flatten()
            .map(|r| (r.id.as_str(), 0))
            .collect();
        scopes(doc, |_, nodes| {
            walk(nodes, |node, _| {
                if let Some(n) = counts.get_mut(node.id_str()) {
                    *n += 1;
                }
                Some(())
            })
        })?;
        index.ambiguous = counts
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(id, _)| id.into())
            .collect();
        (index.bytes() <= MAX_BYTES).then_some(index)
    }
    pub(crate) fn routes(&self, name: &str) -> Option<Vec<Arc<Route>>> {
        if self.overfull.contains(name) {
            return None;
        }
        let routes = self.by_token.get(name).map_or(&[][..], Vec::as_slice);
        if routes.iter().any(|r| {
            self.ambiguous.contains(&r.id)
                || matches!(&r.scope,Scope::Page{id,..}if self.duplicate_pages.contains(id))
        }) {
            return None;
        }
        Some(routes.to_vec())
    }
    fn refresh(&mut self, doc: &PenDocument, routes: &[Arc<Route>]) -> Option<()> {
        if routes.is_empty() {
            return Some(());
        }
        let removing: HashSet<usize> = routes.iter().map(|r| Arc::as_ptr(r) as usize).collect();
        self.by_token.retain(|_, entries| {
            entries.retain(|entry| !removing.contains(&(Arc::as_ptr(entry) as usize)));
            !entries.is_empty()
        });
        for route in routes {
            let node = route.node(doc)?;
            let mut names = HashSet::new();
            super::visit_reference_fields(node, &mut |name| {
                names.insert(name);
            });
            for name in names {
                if self.overfull.contains(name) {
                    continue;
                }
                let entries = self.by_token.entry(name.into()).or_default();
                if entries.len() >= MAX_TARGETS {
                    self.overfull.insert(name.into());
                } else {
                    entries.push(route.clone());
                }
            }
        }
        self.by_token.retain(|key, _| !self.overfull.contains(key));
        (self.bytes() <= MAX_BYTES).then_some(())
    }
    fn bytes(&self) -> usize {
        let mut seen = HashSet::new();
        let mut bytes = std::mem::size_of::<Self>()
            + self.by_token.capacity() * (std::mem::size_of::<(String, Vec<Arc<Route>>)>() + 32);
        for (name, refs) in &self.by_token {
            bytes = bytes.saturating_add(
                name.capacity() + refs.capacity() * std::mem::size_of::<Arc<Route>>(),
            );
            for route in refs {
                if seen.insert(Arc::as_ptr(route) as usize) {
                    bytes = bytes.saturating_add(
                        std::mem::size_of::<Route>()
                            + 32
                            + route.id.capacity()
                            + route.path.len() * std::mem::size_of::<usize>()
                            + match &route.scope {
                                Scope::Root => 0,
                                Scope::Page { id, .. } => id.capacity(),
                            },
                    );
                }
            }
        }
        for set in [&self.overfull, &self.ambiguous, &self.duplicate_pages] {
            bytes = bytes.saturating_add(
                set.capacity() * (std::mem::size_of::<String>() + 32)
                    + set.iter().map(String::capacity).sum::<usize>(),
            );
        }
        bytes
    }
}
impl Route {
    pub(crate) fn node<'a>(&self, doc: &'a PenDocument) -> Option<&'a PenNode> {
        let mut nodes = match &self.scope {
            Scope::Root => doc.children.as_slice(),
            Scope::Page { index, id } => {
                let page = doc.pages.as_ref()?.get(*index)?;
                if page.id != *id {
                    return None;
                }
                &page.children
            }
        };
        for (depth, &at) in self.path.iter().enumerate() {
            let node = nodes.get(at)?;
            if depth + 1 == self.path.len() {
                return (node.id_str() == self.id).then_some(node);
            }
            nodes = node.children()?;
        }
        None
    }
    pub(crate) fn node_in_mut<'a>(&self, mut nodes: &'a mut [PenNode]) -> Option<&'a mut PenNode> {
        for (depth, &at) in self.path.iter().enumerate() {
            let node = nodes.get_mut(at)?;
            if depth + 1 == self.path.len() {
                return (node.id_str() == self.id).then_some(node);
            }
            if node.children().is_none() {
                return None;
            }
            nodes = node.children_mut()?.as_mut_slice();
        }
        None
    }
}
