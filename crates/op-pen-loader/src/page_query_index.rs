//! Compact trusted-revision raw-page index. Canonical nodes are never retained.
use super::*;
use jian_ops_schema::node::PenNode;
use op_editor_core::PenNodeExt;
use std::collections::HashMap;

const NONE: u32 = u32::MAX;
#[derive(Clone, Copy)]
struct Location {
    parent: u32,
    child: u32,
    root: u32,
}
pub(crate) struct PageIndex {
    by_id: HashMap<String, u32>,
    locations: Vec<Location>,
    key_heap: usize,
    pub(crate) unique: bool,
    independent: bool,
}
impl PageIndex {
    pub(crate) fn roots(state: &EditorState, page: usize) -> Option<&[PenNode]> {
        match &state.doc.pages {
            Some(pages) => pages.get(page).map(|p| p.children.as_slice()),
            None if page == 0 => Some(state.doc.children.as_slice()),
            None => None,
        }
    }
    pub(super) fn build(roots: &[PenNode]) -> Option<Self> {
        let mut out = Self {
            by_id: HashMap::new(),
            locations: Vec::new(),
            key_heap: 0,
            unique: true,
            independent: true,
        };
        for (index, root) in roots.iter().enumerate() {
            out.independent &= root.base().mask_type.is_none()
                && root
                    .base()
                    .blend_mode
                    .as_ref()
                    .is_none_or(|mode| *mode == jian_ops_schema::style::BlendMode::Normal);
            out.push(root, NONE, 0, index.try_into().ok()?)?;
        }
        Some(out)
    }
    fn push(&mut self, node: &PenNode, parent: u32, child: u32, root: u32) -> Option<()> {
        self.independent &= !matches!(node, PenNode::Ref(_))
            && !node.base().explain.as_deref().is_some_and(|explain| {
                explain.starts_with(op_editor_core::table_filter_contract::TABLE_MARKER)
            });
        let ordinal: u32 = self
            .locations
            .len()
            .try_into()
            .ok()
            .filter(|n| *n != NONE)?;
        self.locations.push(Location {
            parent,
            child,
            root,
        });
        if let Some(previous) = self.by_id.get_mut(node.id_str()) {
            *previous = NONE; // A duplicate query keeps the legacy complete-root fallback.
            self.unique = false;
        } else {
            let key = node.id_str().to_owned();
            self.key_heap = self.key_heap.saturating_add(key.capacity());
            self.by_id.insert(key, ordinal);
        }
        // Negative-cache an unsupported/oversized optimization, never a partial
        // semantic index. The caller retains its complete original fallback.
        if self.storage_bytes() > MAX_BYTES.saturating_sub(4096) {
            return None;
        }
        if let Some(children) = node.children() {
            for (index, child) in children.iter().enumerate() {
                self.push(child, ordinal, index.try_into().ok()?, root)?;
            }
        }
        Some(())
    }
    pub(super) fn storage_bytes(&self) -> usize {
        size_of::<Self>()
            + self.locations.capacity() * size_of::<Location>()
            + self.by_id.capacity() * (size_of::<(String, u32)>() + 1) * 2
            + self.key_heap
    }
    /// Exact whole-layout-root selection for a trusted unchanged page. The
    /// proof shares this index's existing32MiB capacity/retired-Arc ledger.
    pub(crate) fn independent_roots(
        &self,
        root_count: usize,
        ids: &[String],
    ) -> Option<Vec<usize>> {
        if !self.independent || !self.unique || ids.is_empty() {
            return None;
        }
        let mut roots = Vec::with_capacity(ids.len());
        for id in ids {
            if id.is_empty() || id.contains("__") {
                return None;
            }
            let ordinal = *self.by_id.get(id)?;
            roots.push(self.locations.get(ordinal as usize)?.root as usize);
        }
        roots.sort_unstable();
        roots.dedup();
        (roots.len() < root_count).then_some(roots)
    }
    pub(crate) fn contains_unique(&self, id: &str) -> bool {
        self.by_id.get(id).is_some_and(|index| *index != NONE)
    }
    pub(crate) fn select<'a>(&self, roots: &'a [PenNode], item: Option<&str>) -> &'a [PenNode] {
        let selected = item
            .filter(|id| !id.contains("__"))
            .and_then(|id| self.by_id.get(id))
            .filter(|index| **index != NONE)
            .and_then(|index| self.locations.get(*index as usize))
            .and_then(|location| roots.get(location.root as usize));
        selected.map(std::slice::from_ref).unwrap_or(roots)
    }
    /// Validate every borrowed/prepared edge and final id. Resolution may have
    /// expanded refs or removed unknown instances; a mismatch keeps DFS fallback.
    pub(crate) fn path<'a>(&self, root: &'a PenNode, id: &str) -> Option<Vec<&'a PenNode>> {
        let mut ordinal = *self.by_id.get(id)?;
        if ordinal == NONE {
            return None;
        }
        let mut indices = Vec::new();
        loop {
            let location = self.locations.get(ordinal as usize)?;
            if location.parent == NONE {
                break;
            }
            indices.push(location.child as usize);
            ordinal = location.parent;
        }
        let mut path = vec![root];
        for index in indices.into_iter().rev() {
            let current = *path.last()?;
            let next = current.children()?.get(index)?;
            path.push(next);
        }
        (path.last()?.id_str() == id).then_some(path)
    }
}
