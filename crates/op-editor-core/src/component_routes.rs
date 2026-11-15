//! One exact active-forest route; no mutation can interleave its lexical use.
use super::{PenNode, PenNodeExt};
pub(super) fn find(nodes: &[PenNode], id: &str) -> Option<Box<[usize]>> {
    fn visit(nodes: &[PenNode], id: &str, path: &mut Vec<usize>) -> Option<Box<[usize]>> {
        for (index, node) in nodes.iter().enumerate() {
            path.push(index);
            if node.id_str() == id {
                return Some(path.clone().into_boxed_slice());
            }
            if let Some(children) = node.children() {
                if let Some(path) = visit(children, id, path) {
                    return Some(path);
                }
            }
            path.pop();
        }
        None
    }
    visit(nodes, id, &mut Vec::new())
}
pub(super) fn at<'a>(mut nodes: &'a [PenNode], path: &[usize]) -> Option<&'a PenNode> {
    let (last, parents) = path.split_last()?;
    for index in parents {
        nodes = nodes.get(*index)?.children()?;
    }
    nodes.get(*last)
}
pub(super) fn at_mut<'a>(mut nodes: &'a mut [PenNode], path: &[usize]) -> Option<&'a mut PenNode> {
    let (last, parents) = path.split_last()?;
    for index in parents {
        let node = nodes.get_mut(*index)?;
        node.children()?;
        nodes = node.children_mut()?;
    }
    nodes.get_mut(*last)
}
