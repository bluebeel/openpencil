//! Page CRUD mutators — `add_page` / `duplicate_page` /
//! `remove_page` / `rename_page` / `reorder_page` / page switching.
//!
//! `PenDocument.pages` is `Option<Vec<PenPage>>`. A single-page
//! document keeps `pages == None` and edits `doc.children`. The
//! first `add_page` / `duplicate_page` call promotes the document to
//! multi-page: the root `children` migrate into "Page 1" so no
//! nodes are lost.

use crate::command_node::{build_leaf_node, remap_subtree_ids_with_allocator};
use crate::fills::set_primary_fill_hex;
use crate::id_allocator::{IdAllocError, IdAllocator};
use crate::node_id::NodeId;
use crate::pen_node_ext::PenNodeExt;
use crate::state::EditorState;
use crate::walkers;
#[path = "page_allocation.rs"]
mod allocation;
#[path = "page_command_plan.rs"]
mod command_plan;
pub(crate) use command_plan::{CommittedPageCommand, PreparedPageCommand};
use jian_ops_schema::node::PenNode;
use jian_ops_schema::page::PenPage;

/// Display name of the hidden master-store page that holds an imported
/// component library's reusable masters. It is a side store, not a
/// rendered design — kept off the active design page so scaffolding,
/// role/cleanup passes and page scoring never see the masters.
pub const COMPONENTS_PAGE_NAME: &str = "Components";

/// Build a bare page with id / name / children. State + lifecycle
/// default to `None`.
pub(crate) fn make_page(id: String, name: String, children: Vec<PenNode>) -> PenPage {
    PenPage {
        id,
        name,
        children,
        background_color: None,
        state: None,
        lifecycle: None,
    }
}

fn make_blank_page_frame(id: &NodeId) -> Option<PenNode> {
    let mut frame = build_leaf_node("frame", id.as_str(), "Frame", 0, 0, 1200, 800)?;
    set_primary_fill_hex(&mut frame, "#FFFFFF");
    Some(frame)
}

impl EditorState {
    /// Ensure the document is in multi-page form, migrating the root
    /// `children` into "Page 1" when no pages exist. Covers both
    /// `pages: None` (the single-page fallback) and `pages: Some([])`
    /// (legal-but-empty multi-page) — without the empty-vec branch,
    /// any nodes that landed in `doc.children` while `pages` was
    /// `Some([])` (via the read/write fallback in `active_children`)
    /// would be stranded the moment `add_page` minted a fresh Page 1
    /// alongside them.
    fn ensure_pages_with_allocator(
        &mut self,
        allocator: &mut dyn IdAllocator,
        taken: &mut std::collections::HashSet<NodeId>,
    ) -> Result<(), IdAllocError> {
        let needs_init = self.doc.pages.as_ref().is_none_or(|pages| pages.is_empty());
        if needs_init {
            // Mint the page id BEFORE moving the root children out —
            // `max_node_id` must see the nodes that are migrating so
            // the new page id can't collide with one of them.
            let id = allocator.allocate(taken)?;
            let routes = self.prepare_page_root_routes(Some(id.as_str()));
            let root = std::mem::take(&mut self.doc.children);
            self.doc.pages = Some(vec![make_page(id.into(), "Page 1".to_string(), root)]);
            routes.install(self);
        }
        Ok(())
    }

    /// Switch the active page to `idx`. False when out of bounds.
    pub fn set_active_page(&mut self, idx: usize) -> bool {
        if idx >= self.page_count() {
            return false;
        }
        if self.ui.active_page_index == idx {
            return true;
        }
        self.ui.active_page_index = idx;
        self.clear_selection();
        true
    }

    /// Authored infinite-canvas background of the active explicit page.
    /// Legacy single-page documents have no page metadata and report `None`
    /// until the first background write promotes them losslessly.
    pub fn active_page_background_color(&self) -> Option<&str> {
        let pages = self.doc.pages.as_ref()?;
        if pages.is_empty() {
            return None;
        }
        pages
            .get(self.ui.active_page_index.min(pages.len() - 1))
            .and_then(|page| page.background_color.as_deref())
    }

    /// Set or clear the active page's infinite-canvas background. A non-empty
    /// write on a legacy `pages: None` (or legal `Some([])`) document promotes
    /// root children into one explicit page within this single mutation.
    /// `None` and blank strings both mean the old omitted/default state.
    pub fn set_active_page_background_color(&mut self, color: Option<String>) -> bool {
        let Ok(mut allocator) = self.sequential_id_allocator(1) else {
            return false;
        };
        self.set_active_page_background_color_with_allocator(color, &mut allocator)
            .unwrap_or(false)
    }

    /// Allocator-aware form of [`Self::set_active_page_background_color`].
    pub(crate) fn set_active_page_background_color_with_allocator_unobserved(
        &mut self,
        color: Option<String>,
        allocator: &mut dyn IdAllocator,
    ) -> Result<bool, IdAllocError> {
        let color = color.and_then(|color| {
            let color = color.trim();
            (!color.is_empty()).then(|| color.to_string())
        });
        let needs_page = self.doc.pages.as_ref().is_none_or(|pages| pages.is_empty());
        if needs_page {
            if color.is_none() {
                return Ok(false);
            }
            let mut taken = self.collect_node_ids();
            self.ensure_pages_with_allocator(allocator, &mut taken)?;
        }
        let pages = self
            .doc
            .pages
            .as_mut()
            .expect("ensure_pages initialized pages");
        let index = self.ui.active_page_index.min(pages.len() - 1);
        if pages[index].background_color == color {
            return Ok(false);
        }
        pages[index].background_color = color;
        Ok(true)
    }

    /// Append a fresh empty page named `"Page N"` and switch to it.
    /// Returns the new index, or `None` on id overflow.
    pub fn add_page(&mut self) -> Option<usize> {
        self.add_page_with_name(None)
    }

    /// Append a fresh empty page with an optional display name and
    /// switch to it. Empty / whitespace-only custom names are rejected.
    pub fn add_page_with_name(&mut self, name: Option<String>) -> Option<usize> {
        self.add_page_with_name_and_children(name, None)
    }

    /// Append a fresh page with optional caller-provided children and
    /// switch to it. External child ids are always remapped into this
    /// document's id space, matching `InsertSubtree`.
    pub fn add_page_with_name_and_children(
        &mut self,
        name: Option<String>,
        children: Option<Vec<PenNode>>,
    ) -> Option<usize> {
        let mut allocator = self.sequential_id_allocator(1).ok()?;
        self.add_page_with_allocator(name, children, &mut allocator)
            .ok()
            .flatten()
    }

    /// Allocator-aware page insertion used by collaboration sessions.
    pub(crate) fn add_page_with_allocator_unobserved(
        &mut self,
        name: Option<String>,
        children: Option<Vec<PenNode>>,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<usize>, IdAllocError> {
        let custom_name = match name {
            Some(name) if name.trim().is_empty() => return Ok(None),
            Some(name) => Some(name),
            None => None,
        };
        let mut taken = allocator.document_collision_ids(&self.doc);
        let promotion = allocation::Promotion::prepare(self, allocator, &mut taken)?;
        let page_id = allocator.allocate(&mut taken)?;
        let page_children = match children {
            Some(mut children) => {
                remap_subtree_ids_with_allocator(&mut children, allocator, &mut taken)?;
                children
            }
            None => {
                let frame_id = allocator.allocate(&mut taken)?;
                let Some(frame) = make_blank_page_frame(&frame_id) else {
                    return Ok(None);
                };
                vec![frame]
            }
        };
        // Every allocation and catalog preflight succeeds before the first write.
        let new_index = self
            .doc
            .pages
            .as_ref()
            .filter(|pages| !pages.is_empty())
            .map_or(1, Vec::len);
        let page_name = custom_name.unwrap_or_else(|| format!("Page {}", new_index + 1));
        let page = make_page(page_id.into(), page_name, page_children);
        let Some(catalogue) =
            self.prepare_page_catalog_change(Some((&page, new_index, None)), None)
        else {
            return Ok(None);
        };
        let routes = self.prepare_page_root_routes(promotion.id().map(NodeId::as_str));
        promotion.commit(self);
        routes.install(self);
        self.doc.pages.as_mut().unwrap().push(page);
        catalogue.commit_without_history(self);
        self.ui.active_page_index = new_index;
        self.clear_selection();
        Ok(Some(new_index))
    }

    /// Append the reusable masters of an imported component library
    /// onto a dedicated, hidden [`COMPONENTS_PAGE_NAME`] page — NOT the
    /// active design page. Keeps `active_children()` clean (only the
    /// design) so the orchestrator's scaffold + role/cleanup passes are
    /// unaffected, while the masters stay in `doc.pages` where the
    /// document-wide component lookup (`ComponentLibrary::from_document`
    /// + `ref_resolve::resolve_refs_for_canvas`) still finds them.
    ///
    /// Master ids are preserved verbatim (NO remapping) so `ref` nodes
    /// keep resolving to their targets. Masters are deduped by id
    /// against whatever already lives on the components page, so a
    /// re-import is idempotent.
    ///
    /// Returns the number of masters actually appended (post-dedup).
    /// The active page index is preserved: a single-page document is
    /// first migrated so its design becomes page 0, and the components
    /// page is appended after it, so the caller's active page keeps
    /// pointing at the design.
    pub fn append_components_page_masters(&mut self, masters: Vec<PenNode>) -> usize {
        let Ok(mut allocator) = self.sequential_id_allocator(1) else {
            return 0;
        };
        self.append_components_page_masters_with_allocator(masters, &mut allocator)
            .unwrap_or(0)
    }

    /// Allocator-aware form of [`Self::append_components_page_masters`].
    pub(crate) fn append_components_page_masters_with_allocator_unobserved(
        &mut self,
        masters: Vec<PenNode>,
        allocator: &mut dyn IdAllocator,
    ) -> Result<usize, IdAllocError> {
        if masters.is_empty() {
            return Ok(0);
        }
        let active = self.ui.active_page_index;
        let mut taken = allocator.document_collision_ids(&self.doc);
        walkers::collect_ids(&masters, &mut taken);
        let promotion = allocation::Promotion::prepare(self, allocator, &mut taken)?;
        let existing_page = self.doc.pages.as_ref().and_then(|pages| {
            pages
                .iter()
                .position(|page| page.name == COMPONENTS_PAGE_NAME)
        });
        let page_id = if existing_page.is_none() {
            Some(allocator.allocate(&mut taken)?)
        } else {
            None
        };
        let mut existing = existing_page
            .map(|index| {
                self.doc.pages.as_ref().unwrap()[index]
                    .children
                    .iter()
                    .map(|node| node.id_str().to_owned())
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();
        let accepted = masters
            .into_iter()
            .filter(|master| existing.insert(master.id_str().to_owned()))
            .collect::<Vec<_>>();
        let added = accepted.len();
        let largest = accepted.iter().map(walkers::max_id_walk).max();
        let routes = self.prepare_page_root_routes(promotion.id().map(NodeId::as_str));
        promotion.commit(self);
        routes.install(self);
        let pages = self.doc.pages.as_mut().unwrap();
        let page_idx = match existing_page {
            Some(index) => index,
            None => {
                pages.push(make_page(
                    page_id.unwrap().into(),
                    COMPONENTS_PAGE_NAME.to_owned(),
                    Vec::new(),
                ));
                pages.len() - 1
            }
        };
        pages[page_idx].children.extend(accepted);
        self.ui.active_page_index = active;
        if added > 0 {
            allocator.authored_ids_inserted(largest);
        }
        Ok(added)
    }

    /// Duplicate the page at `idx`, inserting the clone after it.
    /// New node ids are minted past `max_node_id`. Switches the
    /// active page to the clone.
    pub fn duplicate_page(&mut self, idx: usize) -> Option<usize> {
        self.duplicate_page_with_name(idx, None)
    }

    /// Duplicate the page at `idx`, optionally overriding the clone's
    /// display name. Empty / whitespace-only custom names are rejected.
    pub fn duplicate_page_with_name(&mut self, idx: usize, name: Option<String>) -> Option<usize> {
        let mut allocator = self.sequential_id_allocator(1).ok()?;
        self.duplicate_page_with_allocator(idx, name, &mut allocator)
            .ok()
            .flatten()
    }

    /// Allocator-aware page duplication used by collaboration sessions.
    pub(crate) fn duplicate_page_with_allocator_unobserved(
        &mut self,
        idx: usize,
        name: Option<String>,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<usize>, IdAllocError> {
        let custom_name = match name {
            Some(name) if name.trim().is_empty() => return Ok(None),
            Some(name) => Some(name),
            None => None,
        };
        let mut taken = allocator.document_collision_ids(&self.doc);
        let promotion = allocation::Promotion::prepare(self, allocator, &mut taken)?;
        let Some(clone) = allocation::clone_page(self, idx, custom_name, allocator, &mut taken)?
        else {
            return Ok(None);
        };
        let source = self
            .doc
            .pages
            .as_ref()
            .filter(|pages| !pages.is_empty())
            .map_or(self.doc.children.as_slice(), |pages| {
                pages[idx].children.as_slice()
            });
        let new_index = idx + 1;
        let Some(catalogue) =
            self.prepare_page_catalog_change(Some((&clone, new_index, Some(source))), None)
        else {
            return Ok(None);
        };
        let routes = self.prepare_page_root_routes(promotion.id().map(NodeId::as_str));
        promotion.commit(self);
        routes.install(self);
        self.doc.pages.as_mut().unwrap().insert(new_index, clone);
        catalogue.commit_without_history(self);
        self.ui.active_page_index = new_index;
        self.clear_selection();
        Ok(Some(new_index))
    }

    /// Set a page's name directly. Rejects out-of-range indices and
    /// empty / whitespace-only names.
    pub fn rename_page(&mut self, idx: usize, name: impl Into<String>) -> bool {
        let name: String = name.into();
        if name.trim().is_empty() {
            return false;
        }
        let Some(pages) = self.doc.pages.as_mut() else {
            // Single-page document: only index 0 is valid, and the
            // implicit page has no name field to write.
            return false;
        };
        let Some(page) = pages.get_mut(idx) else {
            return false;
        };
        page.name = name;
        true
    }

    /// Move a page from `from` to `to`. `to` is clamped into range.
    /// Keeps `active_page_index` pointing at the same logical page.
    pub fn reorder_page(&mut self, from: usize, to: usize) -> bool {
        let Some(pages) = self.doc.pages.as_mut() else {
            return false;
        };
        if from >= pages.len() {
            return false;
        }
        let to = to.min(pages.len().saturating_sub(1));
        if from == to {
            return false;
        }
        if from < to {
            pages[from..=to].rotate_left(1);
        } else {
            pages[to..=from].rotate_right(1);
        }
        let active = self.ui.active_page_index;
        if active == from {
            self.ui.active_page_index = to;
        } else if from < active && to >= active {
            self.ui.active_page_index = active - 1;
        } else if from > active && to <= active {
            self.ui.active_page_index = active + 1;
        }
        true
    }

    /// Swap the page at `idx` with the previous one. No-op at 0.
    pub fn move_page_up(&mut self, idx: usize) -> bool {
        if idx == 0 {
            return false;
        }
        self.reorder_page(idx, idx - 1)
    }

    /// Swap the page at `idx` with the next one. No-op at the end.
    pub fn move_page_down(&mut self, idx: usize) -> bool {
        let count = self.doc.pages.as_ref().map(|p| p.len()).unwrap_or(0);
        if idx + 1 >= count {
            return false;
        }
        self.reorder_page(idx, idx + 1)
    }

    /// Remove the page at `idx`. No-op when out-of-range OR when
    /// only one page remains. Adjusts `active_page_index` + clears
    /// the selection.
    pub fn remove_page(&mut self, idx: usize) -> bool {
        let Some(page) = self
            .doc
            .pages
            .as_ref()
            .filter(|pages| pages.len() > 1)
            .and_then(|pages| pages.get(idx))
        else {
            return false;
        };
        let Some(catalogue) = self.prepare_page_catalog_change(None, Some(page)) else {
            return false;
        };
        if self.remove_page_retained(idx).is_none() {
            return false;
        }
        catalogue.commit_without_history(self);
        true
    }
    /// Same page/UI mutation, but let an exact undo receipt own the removed
    /// page instead of cloning it before the ordinary mutator drops it.
    pub(crate) fn remove_page_retained(&mut self, idx: usize) -> Option<PenPage> {
        let pages = self.doc.pages.as_mut()?;
        if idx >= pages.len() || pages.len() <= 1 {
            return None;
        }
        let removed = pages.remove(idx);
        let len = pages.len();
        if self.ui.active_page_index >= len {
            self.ui.active_page_index = len - 1;
        } else if idx < self.ui.active_page_index {
            self.ui.active_page_index -= 1;
        }
        self.clear_selection();
        Some(removed)
    }

    /// Rename a node by id on the active page. True when the node
    /// was found. Rejects whitespace-only names.
    pub fn rename_node(&mut self, id: &NodeId, name: impl Into<String>) -> bool {
        let name: String = name.into();
        if name.trim().is_empty() {
            return false;
        }
        let Some(node) = walkers::find_node_mut(self.active_children_mut(), id) else {
            return false;
        };
        use crate::pen_node_ext::PenNodeExt;
        node.base_mut().name = Some(name);
        true
    }
}
