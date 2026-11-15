//! Fresh canonical preorder traversal with O(depth) scratch and no recursive
//! stack. Wide sibling lists remain borrowed iterators, never a node queue.
use jian_ops_schema::node::PenNode;
use op_editor_core::PenNodeExt;
use std::slice::Iter;

pub(crate) struct Entry<'a> {
    pub node: &'a PenNode,
    pub parent: &'a str,
    pub depth: usize,
}

pub(crate) struct Walk<'a> {
    siblings: Iter<'a, PenNode>,
    parent: &'a str,
    depth: usize,
    ancestors: Vec<(Iter<'a, PenNode>, &'a str, usize)>,
}

pub(crate) fn nodes(roots: &[PenNode]) -> Walk<'_> {
    Walk {
        siblings: roots.iter(),
        parent: "",
        depth: 0,
        ancestors: Vec::new(),
    }
}

impl<'a> Iterator for Walk<'a> {
    type Item = Entry<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(node) = self.siblings.next() {
                let entry = Entry {
                    node,
                    parent: self.parent,
                    depth: self.depth,
                };
                if let Some(children) = node.children().filter(|children| !children.is_empty()) {
                    let siblings = std::mem::replace(&mut self.siblings, children.iter());
                    self.ancestors.push((siblings, self.parent, self.depth));
                    self.parent = node.id_str();
                    self.depth += 1;
                }
                return Some(entry);
            }
            let (siblings, parent, depth) = self.ancestors.pop()?;
            self.siblings = siblings;
            self.parent = parent;
            self.depth = depth;
        }
    }
}
