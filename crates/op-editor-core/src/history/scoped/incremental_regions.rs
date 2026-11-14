//! Original root order plus once-per-touched-root before-images. No synthetic
//! document is exposed to a command, read, ref resolver, or custom callback.
use super::*;
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub(super) struct PendingRegions {
    pub(super) scope: Scope,
    pub(super) order: Vec<String>,
    pub(super) old: HashMap<String, PenNode>,
    page: Option<PenPage>,
}
impl PendingRegions {
    pub(super) fn capture(
        state: &EditorState,
        scope: Scope,
    ) -> Result<Option<Self>, ScopedHistoryError> {
        let roots = patch::roots(&state.doc, &scope)?;
        let mut seen = HashSet::new();
        let mut order = Vec::with_capacity(roots.len());
        for root in roots {
            let id = crate::PenNodeExt::id_str(root);
            if id.is_empty() || !seen.insert(id) {
                return Ok(None);
            }
            order.push(id.to_owned());
        }
        let page = match &scope {
            Scope::Root => None,
            Scope::Page { index, .. } => {
                Some(page_metadata(&state.doc.pages.as_ref().unwrap()[*index]))
            }
        };
        Ok(Some(Self {
            scope,
            order,
            old: HashMap::new(),
            page,
        }))
    }
    pub(super) fn capture_targets(
        &mut self,
        state: &EditorState,
        specs: &[TargetSpec],
    ) -> Result<(), ScopedHistoryError> {
        let roots = patch::roots(&state.doc, &self.scope)?;
        for spec in specs.iter().filter(|spec| spec.scope == self.scope) {
            if let Some(index) = spec.path.first() {
                let root = roots
                    .get(*index)
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
                self.retain(root);
            } else if let Some(splice) = &spec.splice {
                let end = splice
                    .index
                    .checked_add(splice.remove)
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
                for root in roots
                    .get(splice.index..end)
                    .ok_or(ScopedHistoryError::NodePathChanged)?
                {
                    self.retain(root);
                }
            } else {
                return Err(ScopedHistoryError::NodePathChanged);
            }
        }
        Ok(())
    }
    fn retain(&mut self, root: &PenNode) {
        let id = crate::PenNodeExt::id_str(root);
        // A root created in this transaction has no original before-image.
        // Its removal on rollback is represented by the original root order.
        if !self.old.contains_key(id) && self.order.iter().any(|original| original == id) {
            self.old.insert(id.to_owned(), root.clone());
        }
    }
    pub(super) fn validate(
        &self,
        doc: &jian_ops_schema::PenDocument,
    ) -> Result<(), ScopedHistoryError> {
        let roots = patch::roots(doc, &self.scope)?;
        let mut present = HashSet::new();
        for root in roots {
            let id = crate::PenNodeExt::id_str(root);
            if id.is_empty() || !present.insert(id) {
                return Err(ScopedHistoryError::NodePathChanged);
            }
        }
        if self
            .order
            .iter()
            .any(|id| !self.old.contains_key(id) && !present.contains(id.as_str()))
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        Ok(())
    }
    /// A broader command is about to execute. Reconstruct the exact initial
    /// forest from retained touched roots and still-unchanged live roots.
    pub(super) fn into_full(
        mut self,
        state: &EditorState,
    ) -> Result<CapturedForest, ScopedHistoryError> {
        self.validate(&state.doc)?;
        let live: HashMap<_, _> = patch::roots(&state.doc, &self.scope)?
            .iter()
            .map(|node| (crate::PenNodeExt::id_str(node), node))
            .collect();
        let mut nodes = Vec::with_capacity(self.order.len());
        for id in &self.order {
            nodes.push(match self.old.remove(id) {
                Some(old) => old,
                None => (*live[id.as_str()]).clone(),
            });
        }
        Ok(match self.scope {
            Scope::Root => CapturedForest::Root(nodes),
            Scope::Page { index, .. } => {
                let mut page = self.page.unwrap();
                page.children = nodes;
                CapturedForest::Page { index, page }
            }
        })
    }
    pub(super) fn restore(mut self, state: &mut EditorState) -> Result<(), ScopedHistoryError> {
        // Validate before taking any live nodes. A failed proof must never
        // discard an untouched forest while trying a legacy retry.
        self.validate(&state.doc)?;
        let roots = patch::roots_mut(&mut state.doc, &self.scope)?;
        let mut current: HashMap<_, _> = mem::take(roots)
            .into_iter()
            .map(|node| (crate::PenNodeExt::id_str(&node).to_owned(), node))
            .collect();
        *roots = self
            .order
            .iter()
            .map(|id| self.old.remove(id).or_else(|| current.remove(id)).unwrap())
            .collect();
        // Audited narrow commands cannot alter page metadata; preserving this
        // checkpoint is cheap and makes the invariant explicit on abort.
        if let (Scope::Page { index, .. }, Some(mut page)) = (self.scope, self.page) {
            page.children = mem::take(&mut state.doc.pages.as_mut().unwrap()[index].children);
            state.doc.pages.as_mut().unwrap()[index] = page;
        }
        Ok(())
    }
    pub(super) fn compact(
        self,
        state: &EditorState,
        out: &mut Vec<Patch>,
    ) -> Result<(), ScopedHistoryError> {
        self.validate(&state.doc)?;
        let current = patch::roots(&state.doc, &self.scope)?;
        patch::compact_incremental_roots(self.scope, self.order, self.old, current, out);
        Ok(())
    }
}
