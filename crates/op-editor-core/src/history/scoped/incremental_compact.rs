//! Consume retained root before-images directly into existing history patches.
//! No new exchange/actor/rebase/retention format is introduced by live batches.
use super::*;

pub(super) fn compact_incremental_roots(
    scope: Scope,
    order: Vec<String>,
    mut old: HashMap<String, PenNode>,
    current: &[PenNode],
    out: &mut Vec<Patch>,
) {
    let positions: HashMap<&str, usize> = current
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id_str(), index))
        .collect();
    let expected: Vec<String> = current
        .iter()
        .map(|node| node.id_str().to_owned())
        .collect();
    let mut removed = Vec::new();
    for id in &order {
        if let Some(before) = old.remove(id) {
            if let Some(index) = positions.get(id.as_str()) {
                owned_compact::node(before, &current[*index], &scope, &mut vec![*index], out);
            } else {
                removed.push(SharedNode::capture_owned(before));
            }
        }
    }
    if order != expected {
        out.push(Patch::Order(OrderPatch {
            scope,
            parent: Box::new([]),
            parent_id: None,
            old_order: order,
            expected_ids: expected,
            old: removed,
        }));
    }
}
