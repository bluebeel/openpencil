//! Generation cleanup is confined to new subtrees or an explicit refine target.
//! Ordinary property edits, copy/paste, import, undo and redo keep authored data.
use jian_ops_schema::node::PenNode;
use op_editor_core::{EditorCommand as C, EditorState, NodeId, PenNodeExt};
use std::collections::{BTreeMap, HashSet};

pub(crate) fn generated_tool(tool: &str) -> bool {
    matches!(
        tool,
        "batch_design" | "design_skeleton" | "design_content" | "design_refine"
    )
}

enum BeforeInsertion {
    Append(usize),
    Identifiers(HashSet<String>),
}
struct Insertion {
    page: usize,
    parent: NodeId,
    before: BeforeInsertion,
}
#[derive(Default)]
pub(super) struct RepairPlan {
    insertions: Vec<Insertion>,
    explicit: Vec<(usize, String)>,
    append_only: bool,
}
impl RepairPlan {
    pub(super) fn is_empty(&self) -> bool {
        self.insertions.is_empty() && self.explicit.is_empty()
    }
    pub(super) fn capture(state: &EditorState, command: &C, enabled: bool) -> Self {
        let mut plan = Self::default();
        if enabled
            && !state.editor_ui.preserve_authored_geometry
            && !state.editor_ui.workspace.active
        {
            plan.append_only = append_only(command);
            let mut active = state.ui.active_page_index;
            plan.collect(state, command, &mut active);
        }
        plan
    }
    fn collect(&mut self, state: &EditorState, command: &C, active: &mut usize) {
        match command {
            C::Batch { commands } => {
                for command in commands {
                    self.collect(state, command, active);
                }
            }
            C::SetActivePage { index } => *active = *index as usize,
            C::InsertSubtree {
                parent_id, page_id, ..
            }
            | C::InsertAuthoredSubtree {
                parent_id, page_id, ..
            }
            | C::InsertAuthoredSubtreePreservingRoots {
                parent_id, page_id, ..
            } => {
                let Some(page) = page_index(state, page_id.as_deref(), *active) else {
                    return;
                };
                if !self
                    .insertions
                    .iter()
                    .any(|item| item.page == page && item.parent == *parent_id)
                {
                    let children = insertion_children(state, page, parent_id);
                    let before = if self.append_only {
                        BeforeInsertion::Append(children.len())
                    } else {
                        BeforeInsertion::Identifiers(
                            children
                                .iter()
                                .map(|node| node.id_str().to_owned())
                                .collect(),
                        )
                    };
                    self.insertions.push(Insertion {
                        page,
                        parent: parent_id.clone(),
                        before,
                    });
                }
            }
            C::RefineDesign {
                root_id, page_id, ..
            } => {
                if let Some(page) = page_index(state, page_id.as_deref(), *active) {
                    // Refine may remint a duplicate target id. An ambiguous
                    // authored id must never redirect heuristic cleanup to its
                    // untouched sibling after that remint. Core refine still
                    // runs; skip only this optional post-generation pass.
                    if count_id(roots(state, page), root_id.as_str()) == 1 {
                        self.explicit.push((page, root_id.as_str().to_owned()));
                    }
                }
            }
            _ => {}
        }
    }
    pub(super) fn apply(self, state: &mut EditorState, before_revision: u64) -> bool {
        if state.document_revision() == before_revision {
            return false;
        }
        if self.append_only {
            return self.apply_appended(state);
        }
        let mut targets: BTreeMap<usize, HashSet<String>> = BTreeMap::new();
        for insertion in self.insertions {
            let BeforeInsertion::Identifiers(before) = insertion.before else {
                unreachable!("append plan handled above")
            };
            for node in insertion_children(state, insertion.page, &insertion.parent) {
                if !before.contains(node.id_str()) {
                    targets
                        .entry(insertion.page)
                        .or_default()
                        .insert(node.id_str().to_owned());
                }
            }
        }
        for (page, id) in self.explicit {
            targets.entry(page).or_default().insert(id);
        }
        let active = state.ui.active_page_index;
        let mut layout_changed = false;
        for (page, targets) in targets {
            state.ui.active_page_index = page;
            layout_changed |= repair_selected(state, &targets);
        }
        state.ui.active_page_index = active;
        layout_changed
    }
    fn apply_appended(self, state: &mut EditorState) -> bool {
        let mut targets: BTreeMap<usize, Vec<Vec<usize>>> = BTreeMap::new();
        for insertion in self.insertions {
            let BeforeInsertion::Append(before) = insertion.before else {
                unreachable!("append-only capture")
            };
            let roots = roots(state, insertion.page);
            let Some(parent) = insertion_parent_path(roots, &insertion.parent) else {
                continue;
            };
            let children = insertion_children(state, insertion.page, &insertion.parent);
            for index in before..children.len() {
                let mut path = parent.clone();
                path.push(index);
                targets.entry(insertion.page).or_default().push(path);
            }
        }
        let active = state.ui.active_page_index;
        let mut changed = false;
        for (page, mut paths) in targets {
            // A generated ancestor owns its generated descendants. Sorting gives
            // the same authored preorder as the legacy id walk, without walking
            // any untouched root or descendant to rediscover known positions.
            paths.sort_unstable();
            let mut selected: Vec<Vec<usize>> = Vec::with_capacity(paths.len());
            for path in paths {
                if selected
                    .last()
                    .is_none_or(|parent| !path.starts_with(parent))
                {
                    selected.push(path);
                }
            }
            state.ui.active_page_index = page;
            changed |= repair_paths(state, selected);
        }
        state.ui.active_page_index = active;
        changed
    }
}

// These commands only append, and never remove/reorder/remint an existing
// sibling. The whole Batch must satisfy this proof; mixed programs retain the
// existing identifier-difference capture. Empty-root replacement is excluded.
fn append_only(command: &C) -> bool {
    match command {
        C::Batch { commands } => commands.iter().all(append_only),
        C::SetActivePage { .. } | C::InsertAuthoredSubtreePreservingRoots { .. } => true,
        C::InsertSubtree { parent_id, .. } | C::InsertAuthoredSubtree { parent_id, .. } => {
            parent_id.is_real()
        }
        _ => false,
    }
}
fn insertion_parent_path(nodes: &[PenNode], parent: &NodeId) -> Option<Vec<usize>> {
    if !parent.is_real() {
        return Some(Vec::new());
    }
    if let Some(index) = nodes
        .iter()
        .position(|node| node.id_str() == parent.as_str())
    {
        return Some(vec![index]);
    }
    for (index, node) in nodes.iter().enumerate() {
        if let Some(path) = node
            .children()
            .and_then(|children| insertion_parent_path(children, parent))
        {
            let mut found = Vec::with_capacity(path.len() + 1);
            found.push(index);
            found.extend(path);
            return Some(found);
        }
    }
    None
}

fn page_index(state: &EditorState, raw: Option<&str>, active: usize) -> Option<usize> {
    let raw = raw.map(str::trim).filter(|raw| !raw.is_empty());
    let Some(pages) = state.doc.pages.as_ref().filter(|pages| !pages.is_empty()) else {
        return raw.map_or(Some(0), |raw| (raw == "0").then_some(0));
    };
    raw.map_or_else(
        || (active < pages.len()).then_some(active),
        |raw| {
            pages.iter().position(|page| page.id == raw).or_else(|| {
                raw.parse::<usize>()
                    .ok()
                    .filter(|index| *index < pages.len())
            })
        },
    )
}
fn roots(state: &EditorState, page: usize) -> &[PenNode] {
    state
        .doc
        .pages
        .as_ref()
        .filter(|pages| !pages.is_empty())
        .and_then(|pages| pages.get(page))
        .map_or(&state.doc.children, |page| &page.children)
}
fn count_id(nodes: &[PenNode], id: &str) -> usize {
    let mut count = 0;
    for node in nodes {
        count += usize::from(node.id_str() == id);
        if let Some(children) = node.children() {
            count += count_id(children, id);
        }
        if count > 1 {
            break;
        }
    }
    count
}
fn insertion_children<'a>(state: &'a EditorState, page: usize, parent: &NodeId) -> &'a [PenNode] {
    let roots = roots(state, page);
    if !parent.is_real() {
        return roots;
    }
    op_editor_core::walkers::find_node(roots, parent)
        .and_then(PenNodeExt::children)
        .map_or(&[], Vec::as_slice)
}

fn collect_paths(
    nodes: &[PenNode],
    targets: &mut HashSet<String>,
    prefix: &mut Vec<usize>,
    out: &mut Vec<Vec<usize>>,
) {
    for (index, node) in nodes.iter().enumerate() {
        if targets.is_empty() {
            break;
        }
        prefix.push(index);
        if targets.remove(node.id_str()) {
            // First authored match, like the command lookup. A selected ancestor
            // owns its subtree: never repair it or a duplicate sibling twice.
            remove_subtree_targets(node, targets);
            out.push(prefix.clone());
        } else if let Some(children) = node.children() {
            collect_paths(children, targets, prefix, out);
        }
        prefix.pop();
    }
}
fn remove_subtree_targets(node: &PenNode, targets: &mut HashSet<String>) {
    if targets.is_empty() {
        return;
    }
    targets.remove(node.id_str());
    if let Some(children) = node.children() {
        for child in children {
            remove_subtree_targets(child, targets);
        }
    }
}
fn children_at<'a>(nodes: &'a mut Vec<PenNode>, path: &[usize]) -> &'a mut Vec<PenNode> {
    let Some((&index, tail)) = path.split_first() else {
        return nodes;
    };
    // Paths came from an immutable traversal and selected ancestors are absent.
    // No insertion accessor is called on an absent child list.
    let children = nodes[index].children_mut().expect("recorded child path");
    children_at(children, tail)
}
fn repair_selected(state: &mut EditorState, targets: &HashSet<String>) -> bool {
    let mut paths = Vec::new();
    collect_paths(
        state.active_children(),
        &mut targets.clone(),
        &mut Vec::new(),
        &mut paths,
    );
    repair_paths(state, paths)
}
fn repair_paths(state: &mut EditorState, paths: Vec<Vec<usize>>) -> bool {
    if paths.is_empty() {
        return false;
    }
    let mut authored = std::mem::take(state.active_children_mut());
    let mut detached = Vec::with_capacity(paths.len());
    // Remove rightmost/deepest first so all recorded positions remain valid.
    for mut path in paths.into_iter().rev() {
        let index = path.pop().unwrap();
        let node = children_at(&mut authored, &path).remove(index);
        path.push(index);
        detached.push((path, Some(node)));
    }
    let mut layout_changed = false;
    for top_level in [true, false] {
        let selected = detached
            .iter_mut()
            .filter(|(path, _)| (path.len() == 1) == top_level)
            .filter_map(|(_, node)| node.take())
            .collect::<Vec<_>>();
        if selected.is_empty() {
            continue;
        }
        *state.active_children_mut() = selected;
        // Nested content is not a screen merely because it is temporarily
        // detached. Keep screen chrome and viewport reflow root-only.
        if top_level {
            super::normalize_mobile_screens_after_apply(state);
        }
        super::normalize_icon_paths_after_apply(state);
        if top_level {
            layout_changed |= op_orchestrator::repair_mobile_trailing_nav_reflow(state);
        }
        layout_changed |=
            op_orchestrator::cleanup_image_slots::materialize_empty_image_fill_slots(state);
        let repaired = std::mem::take(state.active_children_mut());
        let mut repaired = repaired.into_iter();
        for (path, node) in &mut detached {
            if (path.len() == 1) == top_level {
                *node = repaired.next();
            }
        }
        debug_assert!(repaired.next().is_none(), "repairs must retain root count");
    }
    for (mut path, node) in detached.into_iter().rev() {
        let index = path.pop().unwrap();
        children_at(&mut authored, &path).insert(index, node.expect("repair retains every target"));
    }
    *state.active_children_mut() = authored;
    layout_changed
}

#[cfg(test)]
#[path = "write_repairs_tests.rs"]
mod tests;
