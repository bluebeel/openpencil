//! Cheap checkpoint clones and exact, separately deduplicated edge storage.
use super::*;

pub(super) fn reuse<T>(
    current: Vec<Arc<T>>,
    previous: Option<&Arc<Vec<Arc<T>>>>,
) -> Arc<Vec<Arc<T>>> {
    if let Some(old) = previous {
        if current.len() == old.len()
            && current
                .iter()
                .zip(old.iter())
                .all(|(a, b)| Arc::ptr_eq(a, b))
        {
            return old.clone();
        }
    }
    Arc::new(current)
}

pub(super) fn storage<T>(edges: &Arc<Vec<T>>) -> (usize, usize) {
    (
        Arc::as_ptr(edges) as usize,
        std::mem::size_of::<Vec<T>>()
            + 2 * std::mem::size_of::<usize>()
            + edges.capacity() * std::mem::size_of::<T>(),
    )
}

pub(super) fn needs_repair(edges: &[Arc<SharedNode>], id: &NodeId) -> bool {
    edges.iter().any(|entry| {
        entry
            .find_node(id)
            .is_some_and(|n| !matches!(n.metadata(), PenNode::Ref(_)))
    })
}

pub(super) fn repair(edges: &mut Arc<Vec<Arc<SharedNode>>>, id: &NodeId, replacement: &PenNode) {
    if needs_repair(edges, id) {
        for entry in Arc::make_mut(edges) {
            SharedNode::repair(entry, id, replacement);
        }
    }
}
