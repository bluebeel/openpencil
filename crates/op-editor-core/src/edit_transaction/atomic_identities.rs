//! Complete identity presence once, then exact command-local deltas. Counts
//! preserve duplicate-ID behavior; emitted IDs still follow document preorder.
use crate::{
    history::{Scope, ScopedHistoryError, TargetSpec},
    EditorCommand, EditorState, PenNodeExt,
};
use jian_ops_schema::{node::PenNode, PenDocument};
use std::collections::{HashMap, HashSet};

pub(super) struct IdentityTracker {
    counts: HashMap<String, usize>,
    before: HashMap<String, bool>,
    dirty: HashMap<Scope, Option<HashSet<String>>>,
}
pub(super) enum Observation {
    None,
    Splice {
        spec: TargetSpec,
        old_len: usize,
        removed: Vec<String>,
    },
    Forests(Vec<(Scope, Vec<String>)>),
}
impl IdentityTracker {
    pub(super) fn new(doc: &PenDocument) -> Self {
        let mut counts = HashMap::new();
        for page in doc.pages.iter().flatten() {
            add(&mut counts, &page.id);
            visit(&page.children, &mut |id| add(&mut counts, id));
        }
        visit(&doc.children, &mut |id| add(&mut counts, id));
        Self {
            counts,
            before: HashMap::new(),
            dirty: HashMap::new(),
        }
    }
    pub(super) fn begin_step(&mut self) {
        self.before.clear();
        self.dirty.clear();
    }
    pub(super) fn observe_before(
        &mut self,
        state: &EditorState,
        command: &EditorCommand,
        specs: Option<&[TargetSpec]>,
        scopes: &[Scope],
    ) -> Result<Observation, ScopedHistoryError> {
        // A strict metadata plan rejects id/type/children patches and Ref
        // ancestry. It therefore proves identity preservation, even for a
        // broad public command classification such as PatchNodeData.
        if specs.is_some_and(|specs| {
            specs
                .iter()
                .all(|s| !s.subtree && s.splice.is_none() && s.topology.is_none())
        }) || !command.may_change_document_identities()
        {
            return Ok(Observation::None);
        }
        if let Some([spec]) = specs {
            if spec.topology.is_none() {
                if let Some(splice) = &spec.splice {
                    let children = children(&state.doc, spec)?;
                    let end = splice
                        .index
                        .checked_add(splice.remove)
                        .ok_or(ScopedHistoryError::NodePathChanged)?;
                    let removed = children
                        .get(splice.index..end)
                        .ok_or(ScopedHistoryError::NodePathChanged)?;
                    let mut ids = Vec::new();
                    visit(removed, &mut |id| ids.push(id.to_owned()));
                    self.mark_root(state, &spec.scope, &spec.path)?;
                    return Ok(Observation::Splice {
                        spec: spec.clone(),
                        old_len: children.len(),
                        removed: ids,
                    });
                }
            }
        }
        // Unknown identity-changing commands retain their exact, classified
        // forest walk. Other pages/resources are never replaced or filtered.
        let mut observations = Vec::new();
        for scope in scopes {
            let mut ids = Vec::new();
            visit(roots(&state.doc, scope)?, &mut |id| ids.push(id.to_owned()));
            self.dirty.insert(scope.clone(), None);
            observations.push((scope.clone(), ids));
        }
        Ok(Observation::Forests(observations))
    }
    pub(super) fn observe_after(
        &mut self,
        state: &EditorState,
        observation: Observation,
    ) -> Result<(), ScopedHistoryError> {
        match observation {
            Observation::None => (),
            Observation::Splice {
                spec,
                old_len,
                removed,
            } => {
                let splice = spec
                    .splice
                    .as_ref()
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
                let children = children(&state.doc, &spec)?;
                let retained = old_len
                    .checked_sub(splice.remove)
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
                let inserted = children
                    .len()
                    .checked_sub(retained)
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
                if inserted > splice.insert_limit {
                    return Err(ScopedHistoryError::NodePathChanged);
                }
                let end = splice
                    .index
                    .checked_add(inserted)
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
                let added = children
                    .get(splice.index..end)
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
                for id in removed {
                    self.change(&id, false)?;
                }
                let mut ids = Vec::new();
                visit(added, &mut |id| ids.push(id.to_owned()));
                for id in ids {
                    self.change(&id, true)?;
                }
                // Top-level insertions have no parent root in the preimage.
                if spec.path.is_empty() {
                    for node in added {
                        self.mark(&spec.scope, node.id_str());
                    }
                }
            }
            Observation::Forests(forests) => {
                for (scope, old) in forests {
                    for id in old {
                        self.change(&id, false)?;
                    }
                    let mut ids = Vec::new();
                    visit(roots(&state.doc, &scope)?, &mut |id| {
                        ids.push(id.to_owned())
                    });
                    for id in ids {
                        self.change(&id, true)?;
                    }
                }
            }
        }
        Ok(())
    }
    fn change(&mut self, id: &str, inserted: bool) -> Result<(), ScopedHistoryError> {
        if id.is_empty() {
            return Ok(());
        }
        let count = self.counts.get(id).copied().unwrap_or(0);
        self.before.entry(id.to_owned()).or_insert(count != 0);
        if inserted {
            self.counts.insert(
                id.to_owned(),
                count
                    .checked_add(1)
                    .ok_or(ScopedHistoryError::NodePathChanged)?,
            );
        } else if count == 1 {
            self.counts.remove(id);
        } else if count > 1 {
            *self.counts.get_mut(id).unwrap() = count - 1;
        } else {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        Ok(())
    }
    fn mark_root(
        &mut self,
        state: &EditorState,
        scope: &Scope,
        path: &[usize],
    ) -> Result<(), ScopedHistoryError> {
        if let Some(index) = path.first() {
            let root = roots(&state.doc, scope)?
                .get(*index)
                .ok_or(ScopedHistoryError::NodePathChanged)?;
            self.mark(scope, root.id_str());
        }
        Ok(())
    }
    fn mark(&mut self, scope: &Scope, id: &str) {
        if let Some(roots) = self
            .dirty
            .entry(scope.clone())
            .or_insert_with(|| Some(HashSet::new()))
            .as_mut()
        {
            roots.insert(id.to_owned());
        }
    }
    pub(super) fn finish_step(
        &mut self,
        doc: &PenDocument,
    ) -> Result<Vec<String>, ScopedHistoryError> {
        let mut candidates: HashSet<&str> = self
            .before
            .iter()
            .filter_map(|(id, existed)| {
                (!existed && self.counts.contains_key(id)).then_some(id.as_str())
            })
            .collect();
        let mut created = Vec::new();
        let mut scan = |scope: &Scope, nodes: &[PenNode]| {
            let Some(dirty) = self.dirty.get(scope) else {
                return;
            };
            for node in nodes {
                if dirty.as_ref().is_none_or(|ids| ids.contains(node.id_str())) {
                    visit(std::slice::from_ref(node), &mut |id| {
                        if candidates.remove(id) {
                            created.push(id.to_owned());
                        }
                    });
                }
            }
        };
        for (index, page) in doc.pages.iter().flatten().enumerate() {
            scan(
                &Scope::Page {
                    index,
                    id: page.id.clone(),
                },
                &page.children,
            );
        }
        scan(&Scope::Root, &doc.children);
        if !candidates.is_empty() {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        self.before.clear();
        self.dirty.clear();
        Ok(created)
    }
}
fn add(counts: &mut HashMap<String, usize>, id: &str) {
    if !id.is_empty() {
        *counts.entry(id.to_owned()).or_default() += 1;
    }
}
fn visit(nodes: &[PenNode], action: &mut impl FnMut(&str)) {
    let mut stack = vec![nodes.iter()];
    while let Some(siblings) = stack.last_mut() {
        if let Some(node) = siblings.next() {
            if !node.id_str().is_empty() {
                action(node.id_str());
            }
            if let Some(children) = node.children() {
                stack.push(children.iter());
            }
        } else {
            stack.pop();
        }
    }
}
fn roots<'a>(doc: &'a PenDocument, scope: &Scope) -> Result<&'a [PenNode], ScopedHistoryError> {
    match scope {
        Scope::Root => Ok(&doc.children),
        Scope::Page { index, id } => {
            let page = doc
                .pages
                .as_ref()
                .and_then(|p| p.get(*index))
                .ok_or(ScopedHistoryError::PageMissing { index: *index })?;
            if page.id != *id {
                return Err(ScopedHistoryError::PageIdentityChanged { index: *index });
            }
            Ok(&page.children)
        }
    }
}
fn children<'a>(
    doc: &'a PenDocument,
    spec: &TargetSpec,
) -> Result<&'a [PenNode], ScopedHistoryError> {
    let mut children = roots(doc, &spec.scope)?;
    for index in spec.path.iter() {
        let node = children
            .get(*index)
            .ok_or(ScopedHistoryError::NodePathChanged)?;
        children = node.children().map_or(&[], Vec::as_slice);
    }
    Ok(children)
}
