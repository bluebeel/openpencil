//! Resolve membership/first-match identity once per forest, never per selected ID.
use jian_ops_schema::node::PenNode;
use op_editor_core::{EditorState, NodeReadOrder, PenNodeExt, SelectionState};
use std::collections::{HashMap, HashSet};

fn collect<'a>(
    nodes: &'a [PenNode],
    missing: &mut HashSet<&str>,
    found: &mut impl FnMut(&'a PenNode),
) {
    if missing.is_empty() {
        return;
    }
    for node in nodes {
        if missing.remove(node.id_str()) {
            // First preorder identity is exactly walkers::find_node, including
            // duplicate IDs. Do not overwrite it with a later duplicate.
            found(node);
        }
        if missing.is_empty() {
            return;
        }
        if let Some(children) = node.children() {
            collect(children, missing, found);
            if missing.is_empty() {
                return;
            }
        }
    }
}

pub(super) fn roots<'a>(selection: &SelectionState, state: &'a EditorState) -> Vec<&'a PenNode> {
    if selection.set.is_empty() {
        return Vec::new();
    }
    let nodes = state.active_children();
    if let Some(lookup) = state
        .trusted_node_lookup(nodes)
        .filter(|lookup| lookup.is_ready())
    {
        return selection
            .set
            .iter()
            .filter_map(|id| {
                lookup
                    .find(id.as_str(), NodeReadOrder::First)
                    .map(|found| found.node)
            })
            .collect();
    }
    if selection.set.len() <= 1 {
        return selection
            .set
            .iter()
            .filter_map(|id| op_editor_core::walkers::find_node(nodes, id))
            .collect();
    }
    let mut missing = selection
        .set
        .iter()
        .map(|id| id.as_str())
        .collect::<HashSet<_>>();
    let mut found = HashMap::with_capacity(missing.len());
    collect(nodes, &mut missing, &mut |node| {
        found.insert(node.id_str(), node);
    });
    selection
        .set
        .iter()
        .filter_map(|id| found.get(id.as_str()).copied())
        .collect()
}

pub(super) fn prune(selection: &mut SelectionState, state: &EditorState) {
    if selection.set.is_empty() {
        selection.anchor = Default::default();
        return;
    }
    let nodes = state.active_children();
    if let Some(lookup) = state
        .trusted_node_lookup(nodes)
        .filter(|lookup| lookup.is_ready())
    {
        selection
            .set
            .retain(|id| lookup.find(id.as_str(), NodeReadOrder::First).is_some());
    } else if selection.set.len() <= 1 {
        selection
            .set
            .retain(|id| op_editor_core::walkers::find_node(nodes, id).is_some());
    } else {
        let mut missing = selection
            .set
            .iter()
            .map(|id| id.as_str())
            .collect::<HashSet<_>>();
        let mut found = HashSet::with_capacity(missing.len());
        collect(nodes, &mut missing, &mut |node| {
            found.insert(node.id_str());
        });
        drop(missing);
        selection.set.retain(|id| found.contains(id.as_str()));
    }
    // Preserve selection order/duplicates and the previous anchor policy.
    selection.anchor = selection.set.last().cloned().unwrap_or_default();
}
