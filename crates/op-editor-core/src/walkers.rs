//! Free tree-walk helpers for the `EditorState` mutators, operating
//! on the canonical `PenDocument` node tree.
//!
//! shell-core's `document/walkers.rs` walked its own flat `Node`
//! tree; this is the equivalent layer for `jian_ops_schema::PenNode`,
//! reached through the [`PenNodeExt`] uniform-access trait so each
//! walk stays variant-agnostic.

use crate::id_allocator::{IdAllocError, IdAllocator, SequentialIdAllocator};
use crate::node_id::NodeId;
use crate::pen_node_ext::PenNodeExt;
use jian_ops_schema::node::PenNode;
use std::collections::HashSet;
#[path = "walker_preorder.rs"]
mod preorder;
#[path = "walker_mutation_routes.rs"]
mod routes;
#[path = "walker_stack.rs"]
pub(crate) mod stack;

/// Direction for `reorder_selected` — picks which sibling the
/// selected node swaps with in its parent's children vec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReorderDirection {
    /// Towards the front of the paint order (lower index,
    /// `children[0]` is top). Bound to `]`.
    Up,
    /// Towards the back of the paint order (higher index, drawn
    /// underneath). Bound to `[`.
    Down,
}

/// Find a node by id anywhere in a children forest. Returns a shared
/// reference, or `None` when the id resolves to no node.
pub fn find_node<'a>(children: &'a [PenNode], id: &NodeId) -> Option<&'a PenNode> {
    if children.is_empty() {
        return None;
    }
    let mut pending = stack::Stack::<_, 16>::new();
    pending.push(children.iter());
    while let Some(siblings) = pending.last_mut() {
        let Some(node) = siblings.next() else {
            pending.pop();
            continue;
        };
        if node.id_str() == id.as_str() {
            return Some(node);
        }
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            pending.push(children.iter());
        }
    }
    None
}

/// Mutable form of [`find_node`].
pub fn find_node_mut<'a>(children: &'a mut [PenNode], id: &NodeId) -> Option<&'a mut PenNode> {
    if children.is_empty() {
        return None;
    }
    let mut pending = stack::Stack::<_, 16>::new();
    pending.push(children.iter_mut());
    while let Some(siblings) = pending.last_mut() {
        let Some(node) = siblings.next() else {
            pending.pop();
            continue;
        };
        if node.id_str() == id.as_str() {
            return Some(node);
        }
        // Search must never author a previously absent Ref child override.
        if let Some(children) = node
            .children_mut_if_present()
            .filter(|children| !children.is_empty())
        {
            pending.push(children.iter_mut());
        }
    }
    None
}

/// True when `id` resolves to a node anywhere in the forest.
pub fn contains_node(children: &[PenNode], id: &NodeId) -> bool {
    find_node(children, id).is_some()
}

/// True when `target` equals `node` or appears anywhere in its
/// subtree. Used to reject reparenting cycles.
pub fn descendant_contains(node: &PenNode, target: &NodeId) -> bool {
    node.id_str() == target.as_str()
        || node
            .children()
            .is_some_and(|children| find_node(children, target).is_some())
}

/// Remove the first node matching `target` from `children` or any
/// descendant's children. True when something was removed.
pub fn remove_from_children(children: &mut Vec<PenNode>, target: &NodeId) -> bool {
    extract_node(children, target).is_some()
}

/// Drain the node matching `target` out of the forest and return it.
/// Mirrors shell-core's `extract_node` — backs the reorder/reparent
/// extract phase.
pub fn extract_node(children: &mut Vec<PenNode>, target: &NodeId) -> Option<PenNode> {
    let path = routes::find(children, target, |_, _| true)?;
    let (at, parents) = path.split_last()?;
    Some(routes::vec_mut(children, parents)?.remove(*at))
}

/// Insert `node` immediately before `anchor`. `Ok(())` on success;
/// `Err(node)` bounces the payload back on miss.
// `Err(PenNode)` is the bounced-back payload, not an error type.
#[allow(clippy::result_large_err)]
pub fn insert_before_in_children(
    children: &mut Vec<PenNode>,
    anchor: &NodeId,
    node: PenNode,
) -> Result<(), PenNode> {
    let Some(path) = routes::find(children, anchor, |_, _| true) else {
        return Err(node);
    };
    let Some((at, parents)) = path.split_last() else {
        return Err(node);
    };
    let Some(siblings) = routes::vec_mut(children, parents) else {
        return Err(node);
    };
    siblings.insert(*at, node);
    Ok(())
}

/// Insert `node` immediately after `anchor`. `Ok(())` / `Err(node)`.
// `Err(PenNode)` is the bounced-back payload, not an error type.
#[allow(clippy::result_large_err)]
pub fn insert_after_in_children(
    children: &mut Vec<PenNode>,
    anchor: &NodeId,
    node: PenNode,
) -> Result<(), PenNode> {
    let Some(path) = routes::find(children, anchor, |_, _| true) else {
        return Err(node);
    };
    let Some((at, parents)) = path.split_last() else {
        return Err(node);
    };
    let Some(siblings) = routes::vec_mut(children, parents) else {
        return Err(node);
    };
    siblings.insert(*at + 1, node);
    Ok(())
}

/// Append `node` as the LAST child of `parent`. `Ok(())` / `Err(node)`.
/// Fails (bounces the payload) when `parent` is not a container.
// `Err(PenNode)` is the bounced-back payload, not an error type — boxing
// it would be a needless allocation on the hot insert path.
#[allow(clippy::result_large_err)]
pub fn append_into(
    children: &mut [PenNode],
    parent: &NodeId,
    node: PenNode,
) -> Result<(), PenNode> {
    let Some(path) = routes::find(children, parent, |siblings, at| siblings[at].is_container())
    else {
        return Err(node);
    };
    let Some(target) = routes::node_mut(children, &path) else {
        return Err(node);
    };
    let Some(grand) = target.children_mut() else {
        return Err(node);
    };
    grand.push(node);
    Ok(())
}

/// Insert `node` as the FIRST child of `parent`. `Ok(())` / `Err(node)`.
/// Mirrors `append_into` but lands the payload at index 0 so a
/// layer-panel "drop into container" matches TS (`layer-dnd-utils.ts`
/// inserts at index 0 — the top of the child list). Fails (bounces the
/// payload) when `parent` is not a container.
#[allow(clippy::result_large_err)]
pub fn prepend_into(
    children: &mut [PenNode],
    parent: &NodeId,
    node: PenNode,
) -> Result<(), PenNode> {
    let Some(path) = routes::find(children, parent, |siblings, at| siblings[at].is_container())
    else {
        return Err(node);
    };
    let Some(target) = routes::node_mut(children, &path) else {
        return Err(node);
    };
    let Some(grand) = target.children_mut() else {
        return Err(node);
    };
    grand.insert(0, node);
    Ok(())
}

/// Swap the node matching `target` with its next / prev sibling.
pub fn reorder_in_children(
    children: &mut [PenNode],
    target: &NodeId,
    direction: ReorderDirection,
) -> bool {
    let Some(path) = routes::find(children, target, |siblings, at| match direction {
        ReorderDirection::Up => at > 0,
        ReorderDirection::Down => at + 1 < siblings.len(),
    }) else {
        return false;
    };
    let Some((at, parents)) = path.split_last() else {
        return false;
    };
    let Some(siblings) = routes::slice_mut(children, parents) else {
        return false;
    };
    let neighbor = match direction {
        ReorderDirection::Up => *at - 1,
        ReorderDirection::Down => *at + 1,
    };
    siblings.swap(*at, neighbor);
    true
}

/// Insert every descendant id of the forest into `out`.
pub fn collect_ids(children: &[PenNode], out: &mut HashSet<NodeId>) {
    for node in preorder::Nodes::new(children) {
        if let Some(id) = NodeId::new_opt(node.id_str()) {
            out.insert(id);
        }
    }
}

/// Parse the numeric suffix of an editor-minted `n{N}` id. `None`
/// for any id not matching `^n\d+$` (the NONE sentinel or an
/// arbitrary canonical-schema id).
pub fn parse_n_id(id: &str) -> Option<u64> {
    let digits = id.strip_prefix('n')?;
    if digits.is_empty() {
        return None;
    }
    digits.bytes().try_fold(0u64, |value, byte| {
        let digit = byte.checked_sub(b'0').filter(|digit| *digit < 10)?;
        value.checked_mul(10)?.checked_add(u64::from(digit))
    })
}

/// Largest `n{N}` suffix in the subtree (0 when none exists).
pub fn max_id_walk(node: &PenNode) -> u64 {
    preorder::Nodes::new(std::slice::from_ref(node))
        .filter_map(|node| parse_n_id(node.id_str()))
        .max()
        .unwrap_or(0)
}

/// Allocate the next free editor-minted id. Formats `n{*next_id}`,
/// advancing past any candidate already present in `taken`. Returns
/// `None` only on `u64` counter exhaustion.
pub fn alloc_n_id(next_id: &mut u64, taken: &mut HashSet<NodeId>) -> Option<NodeId> {
    let mut allocator = SequentialIdAllocator::new(*next_id);
    let allocated = allocator.allocate(taken).ok();
    *next_id = allocator.next_counter();
    allocated
}

/// Node count in `node`'s subtree, including `node` itself.
pub fn subtree_size(node: &PenNode) -> u64 {
    preorder::Nodes::new(std::slice::from_ref(node)).fold(0u64, |count, _| count.saturating_add(1))
}

/// Deep-clone `node`, minting a fresh `n{N}` id for it and every
/// descendant from `next_id`. Geometry + style copy verbatim.
pub fn deep_clone_with_new_ids(
    node: &PenNode,
    next_id: &mut u64,
    taken: &mut HashSet<NodeId>,
) -> PenNode {
    try_deep_clone_with_new_ids(node, next_id, taken)
        .expect("legacy deep clone preflight must reserve sufficient id headroom")
}

/// Typed standalone form of [`deep_clone_with_new_ids`].
///
/// New code should prefer [`deep_clone_with_allocator`] so a session-owned
/// collaboration namespace can flow through the whole subtree.
pub fn try_deep_clone_with_new_ids(
    node: &PenNode,
    next_id: &mut u64,
    taken: &mut HashSet<NodeId>,
) -> Result<PenNode, IdAllocError> {
    let mut allocator = SequentialIdAllocator::new(*next_id);
    let result = deep_clone_with_allocator(node, &mut allocator, taken);
    *next_id = allocator.next_counter();
    result
}

/// Deep-clone a subtree while allocating every canonical id from `allocator`.
///
/// On exhaustion no partial node is returned and no fallback id is fabricated.
/// Allocator counters may advance on failure; session allocators deliberately
/// never reuse an id attempt.
pub fn deep_clone_with_allocator(
    node: &PenNode,
    allocator: &mut dyn IdAllocator,
    taken: &mut HashSet<NodeId>,
) -> Result<PenNode, IdAllocError> {
    struct Frame<'a> {
        node: PenNode,
        remaining: std::slice::Iter<'a, PenNode>,
        children: Vec<PenNode>,
    }
    // A small number of open ancestors lives inline; deep canonical trees
    // spill to explicit cursors rather than recursive Rust call frames.
    let mut parents = stack::Stack::<Frame<'_>, 4>::new();
    let mut source = node;
    loop {
        let mut cloned = crate::pen_node_ext::clone_without_children(source);
        cloned.base_mut().id = allocator.allocate(taken)?.into();
        if let Some(children) = source.children().filter(|children| !children.is_empty()) {
            let mut remaining = children.iter();
            if let Some(first) = remaining.next() {
                parents.push(Frame {
                    node: cloned,
                    remaining,
                    children: Vec::with_capacity(children.len()),
                });
                source = first;
                continue;
            }
        }
        // Attach a completed child, then finish as many exhausted parents as
        // needed before visiting the next sibling. Allocation remains preorder.
        loop {
            let Some(parent) = parents.last_mut() else {
                return Ok(cloned);
            };
            parent.children.push(cloned);
            if let Some(next) = parent.remaining.next() {
                source = next;
                break;
            }
            let parent = parents.pop().expect("open clone parent");
            cloned = parent.node;
            *cloned
                .children_mut()
                .expect("source has canonical children") = parent.children;
        }
    }
}

#[cfg(test)]
#[path = "walkers_clone_tests.rs"]
mod clone_tests;

/// Translate the subtree rooted at `node` by `(dx, dy)` document px.
///
/// Only the root's own `x` / `y` move. Child coords in the canonical
/// schema are PARENT-RELATIVE (jian-core's layout engine resolves a
/// child AABB against its parent origin), so shifting the root alone
/// carries the whole subtree visually. Cascading the delta into
/// descendants — as a doc-absolute model would need — double-shifts
/// every nested free-layout child (proven by layout probe), and TS
/// agrees: the drag commit writes only the dragged node
/// (`skia-interaction-select.ts` `docStore.updateNode(orig.id,
/// {x: orig.x + dx, y: orig.y + dy})`; same in pen-engine's
/// `select-handler.ts`). It would also materialize explicit `x` / `y`
/// onto flex flow children, detaching them from auto-layout flow.
pub fn translate_subtree(node: &mut PenNode, dx: f64, dy: f64) {
    let base = node.base_mut();
    base.x = Some(base.x.unwrap_or(0.0) + dx);
    base.y = Some(base.y.unwrap_or(0.0) + dy);
}

/// Locate `target`'s parent container anywhere in the forest.
/// `Some((None, idx))` when `target` sits at the top level,
/// `Some((Some(parent_id), idx))` for a nested target, `None` when
/// absent from the forest entirely.
pub fn find_parent_and_index(
    children: &[PenNode],
    target: &NodeId,
) -> Option<(Option<NodeId>, usize)> {
    let path = routes::find(children, target, |_, _| true)?;
    let (at, parents) = path.split_last()?;
    let mut siblings = children;
    let mut parent = None;
    for index in parents {
        let node = siblings.get(*index)?;
        // Preserve the old recursive fallback through an invalid empty parent
        // ID to its nearest nonempty ancestor, not a different duplicate node.
        if !node.id_str().is_empty() {
            parent = Some(node.id_str());
        }
        siblings = node.children()?;
    }
    Some((parent.and_then(NodeId::new_opt), *at))
}

/// Insert `node` into the container `parent` (`None` = top level) at
/// `index` (clamped to the child count; `None` = append). False when
/// the parent container no longer exists or cannot hold children.
pub fn insert_into_parent(
    children: &mut Vec<PenNode>,
    parent: Option<&NodeId>,
    index: Option<usize>,
    node: PenNode,
) -> bool {
    let Some(parent_id) = parent else {
        let at = index.unwrap_or(children.len()).min(children.len());
        children.insert(at, node);
        return true;
    };
    let Some(container) = find_node_mut(children, parent_id) else {
        return false;
    };
    let Some(slot) = container.children_mut() else {
        return false;
    };
    let at = index.unwrap_or(slot.len()).min(slot.len());
    slot.insert(at, node);
    true
}

/// True when `target`'s immediate parent (anywhere in the forest) is an
/// auto-layout (flex) container — i.e. `target` is positioned by the
/// layout engine, not by stored `x` / `y`.
///
/// Such a node must not be free-dragged: writing `x` / `y` onto it (even
/// the tiny delta of a jittered click) flips it to `Position::Absolute`
/// in jian-core and detaches it from its parent's flex flow, collapsing
/// the siblings. A drag of an auto-layout child is therefore a no-op
/// (reorder-on-drag is a separate, future affordance). Top-level nodes
/// and children of free-layout parents are NOT affected.
pub fn is_flow_child_of_flex(children: &[PenNode], target: &NodeId) -> bool {
    for child in children {
        if let Some(grand) = child.children() {
            if grand.iter().any(|c| c.id_str() == target.as_str()) {
                return child.is_auto_layout_container();
            }
            if is_flow_child_of_flex(grand, target) {
                return true;
            }
        }
    }
    false
}

/// True when any ancestor of `target` within the forest is also in
/// `set` — used by `translate_selected` to dedupe a double-shift.
pub fn is_ancestor_in_set(children: &[PenNode], target: &NodeId, set: &[NodeId]) -> bool {
    if children.is_empty() || set.is_empty() {
        return false;
    }
    let selected: HashSet<&str> = set.iter().map(NodeId::as_str).collect();
    let mut pending = stack::Stack::<_, 16>::new();
    pending.push((children.iter(), false));
    while let Some((siblings, ancestor_selected)) = pending.last_mut() {
        let Some(node) = siblings.next() else {
            pending.pop();
            continue;
        };
        if node.id_str() == target.as_str() && *ancestor_selected {
            return true;
        }
        // An ancestor with the target's identical ID was explicitly excluded
        // by the legacy predicate, including malformed duplicate-ID trees.
        let inherited = *ancestor_selected
            || (node.id_str() != target.as_str() && selected.contains(node.id_str()));
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            pending.push((children.iter(), inherited));
        }
    }
    false
}

/// Single-pass replacement for the old `is_flow_child_of_flex` +
/// `is_ancestor_in_set` + `find_node_mut` triple (one full-tree walk
/// PER selected id). This descends the forest exactly once, carrying
/// two contexts down the recursion instead of recomputing them from
/// scratch for every id:
///
/// - `parent_is_flex` — whether the CURRENT node's immediate parent is
///   an auto-layout (flex) container. Mirrors `is_flow_child_of_flex`,
///   which only cares about the immediate parent, not any ancestor —
///   so this is recomputed fresh (from the node just visited) at each
///   recursion level, never accumulated.
/// - `ancestor_in_set` — whether ANY proper ancestor's id is in
///   `editable`. Mirrors `is_ancestor_in_set`'s top-down ancestor-chain
///   search. Accumulates via OR as the recursion descends, and is
///   evaluated BEFORE folding in the current node's own membership (a
///   node is never its own ancestor).
///
/// A node translates iff its id is in `editable` AND neither guard
/// applies — matching `translate_selected`'s per-id skip order (flex
/// check first, then the ancestor dedup) exactly. `editable` already
/// reflects the `is_editable` pre-filter (hidden / locked ids excluded
/// by the caller), so this walk never re-checks visibility / lock
/// state itself.
///
/// Recursion is gated on the IMMUTABLE [`PenNodeExt::children`] check
/// first, only reaching for [`PenNodeExt::children_mut`] when that
/// already reports `Some`. `children_mut` eagerly upgrades a
/// container-capable node's `children: None` to `Some(vec![])`
/// (`Option::get_or_insert_with`) — harmless for the few nodes
/// `find_node_mut`'s per-id search happened to pass over in the old
/// three-walk body, but this walk visits every node in the forest
/// exactly once, so calling `children_mut` unconditionally here would
/// silently materialize an empty `children` list on every leaf
/// Rectangle/Frame in the document on every drag frame — bloating the
/// in-memory doc and any subsequent `.op` serialization. A node with
/// no children has nothing left to translate inside it either way, so
/// skipping it is both cheaper and correct.
pub fn translate_editable_subtree(
    children: &mut [PenNode],
    editable: &HashSet<&str>,
    dx: f64,
    dy: f64,
    parent_is_flex: bool,
    ancestor_in_set: bool,
) -> bool {
    if children.is_empty() || editable.is_empty() || ancestor_in_set {
        return false;
    }
    let mut moved = false;
    let mut pending = stack::Stack::<_, 16>::new();
    pending.push((children.iter_mut(), parent_is_flex));
    while let Some((siblings, parent_is_flex)) = pending.last_mut() {
        let Some(node) = siblings.next() else {
            pending.pop();
            continue;
        };
        let in_set = editable.contains(node.id_str());
        if in_set {
            if !*parent_is_flex {
                translate_subtree(node, dx, dy);
                moved = true;
            }
            // All proper descendants are excluded by ancestor-in-set even
            // when this selected node itself is a flex child and cannot move.
            continue;
        }
        if node.children().is_some_and(|children| !children.is_empty()) {
            let node_is_flex = node.is_auto_layout_container();
            if let Some(children) = node.children_mut() {
                pending.push((children.iter_mut(), node_is_flex));
            }
        }
    }
    moved
}

/// First duplicate id found in the forest, or `None` when all ids
/// are unique.
pub fn find_duplicate(children: &[PenNode], seen: &mut HashSet<String>) -> Option<NodeId> {
    let mut pending = stack::Stack::<_, 16>::new();
    if !children.is_empty() {
        pending.push(children.iter());
    }
    while let Some(siblings) = pending.last_mut() {
        let Some(node) = siblings.next() else {
            pending.pop();
            continue;
        };
        if !seen.insert(node.id_str().to_string()) {
            if let Some(id) = NodeId::new_opt(node.id_str()) {
                return Some(id);
            }
            // The old recursive call returned None immediately on a duplicate
            // empty/sentinel ID: skip the rest of exactly this sibling scope,
            // then let its parent's next sibling continue.
            pending.pop();
            continue;
        }
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            pending.push(children.iter());
        }
    }
    None
}

#[cfg(test)]
#[path = "walkers_presence_tests.rs"]
mod presence_tests;
