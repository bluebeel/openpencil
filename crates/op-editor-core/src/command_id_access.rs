//! Exact mutable access through the current command authority. Public/raw
//! callers retain fresh traversal and the historical duplicate-ID priorities.
use super::{list_mut, EditorState, NodeId, Order, PenNode, Scope};
fn node_mut<'a>(
    state: &'a mut EditorState,
    scope: &Scope,
    path: &[usize],
) -> Option<&'a mut PenNode> {
    let (slot, parents) = path.split_last()?;
    list_mut(&mut state.doc, scope, parents, false)?.get_mut(*slot)
}
impl EditorState {
    pub(crate) fn command_find_node_mut(
        &mut self,
        scope: &Scope,
        id: &NodeId,
    ) -> Option<&mut PenNode> {
        match self.command_lookup_path(scope, id, Order::Preorder) {
            Some(Some(path)) => node_mut(self, scope, &path),
            Some(None) => None,
            None => crate::walkers::find_node_mut(list_mut(&mut self.doc, scope, &[], false)?, id),
        }
    }
    pub(crate) fn command_replace_node(
        &mut self,
        scope: &Scope,
        id: &NodeId,
        replacement: PenNode,
    ) -> bool {
        if let Some(path) = self.command_lookup_path(scope, id, Order::SiblingFirst) {
            let Some(node) = path.and_then(|path| node_mut(self, scope, &path)) else {
                return false;
            };
            *node = replacement;
            return true;
        }
        let Some(roots) = list_mut(&mut self.doc, scope, &[], false) else {
            return false;
        };
        crate::command_node::replace_node_in_children(roots, id, &mut Some(replacement))
    }
}
