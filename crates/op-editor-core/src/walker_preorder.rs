//! Complete borrowed preorder with inline shallow cursors and a deep-tree spill.
use super::{stack, PenNode, PenNodeExt};

pub(super) struct Nodes<'a> {
    pending: stack::Stack<std::slice::Iter<'a, PenNode>, 16>,
}
impl<'a> Nodes<'a> {
    pub(super) fn new(nodes: &'a [PenNode]) -> Self {
        let mut pending = stack::Stack::new();
        if !nodes.is_empty() {
            pending.push(nodes.iter());
        }
        Self { pending }
    }
}
impl<'a> Iterator for Nodes<'a> {
    type Item = &'a PenNode;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let siblings = self.pending.last_mut()?;
            let Some(node) = siblings.next() else {
                self.pending.pop();
                continue;
            };
            if let Some(children) = node.children().filter(|children| !children.is_empty()) {
                self.pending.push(children.iter());
            }
            return Some(node);
        }
    }
}
