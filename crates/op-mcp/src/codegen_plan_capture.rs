//! Plan-scoped capture from borrowed canonical trees.
//!
//! The plan store never changes its chunk nodeIds: later hydration only reads
//! those keys. Preserve complete snapshots for those keys without serializing
//! a whole page forest and then cloning every descendant into a second index.
use super::{chunk_node_ids, create_plan_with_index, PlanStoreError};
use crate::read_nodes::node_snapshot_value;
use jian_ops_schema::node::PenNode;
use op_editor_core::PenNodeExt;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub(crate) fn create_plan_for_roots(
    plan: &Value,
    roots: &[PenNode],
) -> Result<Value, PlanStoreError> {
    create_plan_with_index(plan, || {
        let wanted: HashSet<&str> = plan
            .get("chunks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(chunk_node_ids)
            .filter_map(Value::as_str)
            .collect();
        let mut selected = HashMap::with_capacity(wanted.len());
        index_requested(roots, &wanted, &mut selected);
        selected
            .into_iter()
            .filter_map(|(id, node)| {
                let snapshot = node_snapshot_value(node, -1);
                // Match the existing serialized index's eligibility. All normal
                // canonical nodes serialize as objects with their original id.
                (snapshot.get("id").and_then(Value::as_str) == Some(id))
                    .then(|| (id.to_owned(), snapshot))
            })
            .collect()
    })
}

fn index_requested<'a>(
    roots: &'a [PenNode],
    wanted: &HashSet<&str>,
    selected: &mut HashMap<&'a str, &'a PenNode>,
) {
    for node in roots {
        // The existing Value index overwrites at every DFS encounter, so a
        // duplicate authored id deliberately resolves to the LAST node.
        if wanted.contains(node.id_str()) {
            selected.insert(node.id_str(), node);
        }
        if let Some(children) = node.children() {
            index_requested(children, wanted, selected);
        }
    }
}
