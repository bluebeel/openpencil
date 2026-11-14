//! Authored-id subtree insertion for layered design skeletons.

use std::collections::HashSet;

use crate::node_id::NodeId;
use crate::pen_node_ext::PenNodeExt;
use crate::state::EditorState;
use crate::walkers;
use jian_ops_schema::node::PenNode;

impl EditorState {
    pub(crate) fn cmd_insert_authored_subtree_with_root_policy(
        &mut self,
        nodes: Vec<PenNode>,
        parent_id: &NodeId,
        replace_empty_root: bool,
        allocator: &mut dyn crate::IdAllocator,
    ) -> bool {
        if nodes.is_empty() {
            return false;
        }
        let mut nodes = nodes;
        let replacement = replace_empty_root
            .then(|| {
                crate::command_root_replace::prepare_root_frame_replacement(
                    self.active_children(),
                    &mut nodes,
                    parent_id,
                )
            })
            .flatten();
        let scope = self.command_lookup_active_scope();
        let destination = if parent_id.is_real() {
            match self.command_insertion_path(&scope, parent_id) {
                Some(Ok(path)) => Some(path),
                Some(Err(())) => return false,
                None => None,
            }
        } else {
            None
        };
        if parent_id.is_real() && destination.is_none() {
            // Accept any container (matches `cmd_insert_subtree`), including an
            // empty one whose `children` is still `None` — the insert below
            // initializes it via `children_mut()` (`get_or_insert_with`). Using
            // `children().is_some()` here would wrongly REJECT a valid insert
            // under an empty Frame/Group.
            match walkers::find_node(self.active_children(), parent_id) {
                Some(parent) if parent.is_container() => {}
                _ => return false,
            }
        }

        let mut incoming = HashSet::new();
        let mut smallest_numeric = Some(u64::MAX);
        let mut largest_numeric = None;
        if !nodes.iter().all(|node| {
            collect_authored_ids(
                node,
                &mut incoming,
                &mut smallest_numeric,
                &mut largest_numeric,
            )
        }) {
            return false;
        }
        // Fresh sequential ids above the complete current document (including
        // page ids) cannot collide. This borrowed scan avoids cloning/hashing
        // the entire live id set for every program insert. Rich/custom ids keep
        // the exact full collision check, including root-placeholder reuse.
        let provably_fresh = smallest_numeric.is_some_and(|smallest| {
            allocator
                .authored_id_floor(&self.doc)
                .is_some_and(|next| smallest >= next)
        });
        if !provably_fresh {
            let mut live = self.collect_node_ids();
            if let Some(replacement) = replacement.as_ref() {
                live.remove(crate::command_root_replace::replacement_node_id(
                    replacement,
                ));
            }
            if incoming.iter().any(|id| live.contains(id)) {
                return false;
            }
        }

        if let Some(path) = destination {
            if self.command_extend_at(&scope, &path, nodes).is_err() {
                return false;
            }
        } else if parent_id.is_real() {
            let Some(parent) = walkers::find_node_mut(self.active_children_mut(), parent_id) else {
                return false;
            };
            let Some(children) = parent.children_mut() else {
                return false;
            };
            children.extend(nodes);
        } else {
            let roots = self.active_children_mut();
            if let Some(replacement) = replacement.as_ref() {
                if !crate::command_root_replace::remove_root_frame_replacement(roots, replacement) {
                    return false;
                }
            }
            roots.extend(nodes);
        }
        allocator.authored_ids_inserted(largest_numeric);
        true
    }
}

fn collect_authored_ids(
    node: &PenNode,
    incoming: &mut HashSet<NodeId>,
    smallest_numeric: &mut Option<u64>,
    largest_numeric: &mut Option<u64>,
) -> bool {
    // Every authored node MUST carry a valid (non-empty), doc-unique id — that
    // is the whole point of the authored path (vs `cmd_insert_subtree`, which
    // remaps ids and so tolerates an empty/garbage id). Callers that mint ids
    // (e.g. `batch_design`, which assigns a fresh `n{N}` to every node via
    // `remap_subtree_ids_mapping` before emitting `InsertAuthoredSubtree`)
    // always satisfy this; an empty id here is a malformed input and rejected.
    let Some(id) = NodeId::new_opt(node.id_str()) else {
        return false;
    };
    if !incoming.insert(id) {
        return false;
    }
    if let Some(number) = walkers::parse_n_id(node.id_str()) {
        *largest_numeric = Some(largest_numeric.unwrap_or(0).max(number));
    }
    *smallest_numeric = smallest_numeric
        .and_then(|smallest| walkers::parse_n_id(node.id_str()).map(|number| smallest.min(number)));
    node.children()
        .map(|children| {
            children.iter().all(|child| {
                collect_authored_ids(child, incoming, smallest_numeric, largest_numeric)
            })
        })
        .unwrap_or(true)
}
