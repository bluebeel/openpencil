//! One exact parent route is shared by validation and the actual append. The
//! public/fresh path keeps the same first-preorder identity semantics.
use crate::{history::Scope, EditorState, NodeId, PenNodeExt};
pub(super) fn path(state: &EditorState, scope: &Scope, parent: &NodeId) -> Option<Box<[usize]>> {
    if let Some(result) = state.command_insertion_path(scope, parent) {
        return result.ok();
    }
    if !parent.is_real() {
        return Some(Box::new([]));
    }
    let roots = match scope {
        Scope::Root => state.doc.children.as_slice(),
        Scope::Page { index, id } => state
            .doc
            .pages
            .as_ref()?
            .get(*index)
            .filter(|page| page.id == *id)?
            .children
            .as_slice(),
    };
    let mut pending = vec![(roots, 0usize)];
    let mut route = Vec::new();
    while let Some((nodes, next)) = pending.last_mut() {
        let Some(node) = nodes.get(*next) else {
            pending.pop();
            route.pop();
            continue;
        };
        let slot = *next;
        *next += 1;
        route.push(slot);
        if node.id_str() == parent.as_str() {
            return node.is_container().then(|| route.into_boxed_slice());
        }
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            pending.push((children.as_slice(), 0));
        } else {
            route.pop();
        }
    }
    None
}
