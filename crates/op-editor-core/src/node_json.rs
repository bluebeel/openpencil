//! Canonical JSON transforms without repeatedly decoding descendant graphs.

use crate::{pen_node_ext::clone_without_children, PenNodeExt};
use jian_ops_schema::node::PenNode;
use serde_json::Value;

/// Apply a node-local JSON transform to every node, preserving the canonical
/// serde contract. `root` distinguishes the first node from its descendants.
///
/// The transform must not replace `children`: callers with an explicit child
/// replacement use the ordinary full JSON path. Container/type changes remain
/// supported; a non-container result discards children as canonical serde does.
pub fn map_subtree_json(
    source: &PenNode,
    mut transform: impl FnMut(&mut Value, bool),
) -> Result<PenNode, serde_json::Error> {
    fn map(
        source: &PenNode,
        root: bool,
        transform: &mut impl FnMut(&mut Value, bool),
    ) -> Result<PenNode, serde_json::Error> {
        let mut value = serde_json::to_value(clone_without_children(source))?;
        transform(&mut value, root);
        let mut node: PenNode = serde_json::from_value(value)?;
        if let Some(children) = source.children() {
            // Do not materialize absent Ref children. Only a source with an
            // authored child array carries that array into its transformed node.
            if let Some(target) = node.children_mut() {
                target.reserve(children.len());
                for child in children {
                    target.push(map(child, false, transform)?);
                }
            }
        }
        Ok(node)
    }
    map(source, true, &mut transform)
}

#[cfg(test)]
#[path = "node_json_tests.rs"]
mod tests;
