//! Payload preparation reads canonical nodes plus earlier planned insertions.
use super::{
    lookup,
    plan::{Plan, Source},
};
use crate::{
    id_allocator::{IdAllocError, IdAllocator},
    pen_node_ext::clone_without_children,
    walkers, NodeId, PenNodeExt,
};
use jian_ops_schema::node::PenNode;
use std::collections::HashSet;
impl Plan {
    fn clone_original(
        &self,
        node: &PenNode,
        path: &mut Vec<usize>,
        allocator: &mut dyn IdAllocator,
        taken: &mut HashSet<NodeId>,
    ) -> Result<PenNode, IdAllocError> {
        let mut clone = clone_without_children(node);
        clone.base_mut().id = allocator.allocate(taken)?.into();
        if let Some(children) = node.children() {
            let mut output = Vec::with_capacity(children.len() + self.extra_children(path));
            for (index, child) in children.iter().enumerate() {
                self.gap_nodes(path, index, |prepared| {
                    output.push(walkers::deep_clone_with_allocator(
                        prepared, allocator, taken,
                    )?);
                    Ok(())
                })?;
                path.push(index);
                let child = self.clone_original(child, path, allocator, taken)?;
                path.pop();
                output.push(child);
            }
            self.gap_nodes(path, children.len(), |prepared| {
                output.push(walkers::deep_clone_with_allocator(
                    prepared, allocator, taken,
                )?);
                Ok(())
            })?;
            *clone
                .children_mut()
                .expect("canonical source children exist") = output;
        } else {
            debug_assert_eq!(self.extra_children(path), 0);
        }
        Ok(clone)
    }
    pub(super) fn duplicate_payload(
        &self,
        roots: &[PenNode],
        source: &Source,
        allocator: &mut dyn IdAllocator,
        taken: &mut HashSet<NodeId>,
        offset: f64,
    ) -> Result<Option<PenNode>, IdAllocError> {
        let original = self.node(roots, source);
        if let PenNode::Frame(frame) = original {
            if frame.reusable == Some(true) {
                let id = allocator.allocate(taken)?;
                return Ok(serde_json::from_value(
                    serde_json::json!({"type":"ref","id":id.as_str(),"ref":frame.base.id,
                    "x":frame.base.x.unwrap_or(0.0)+offset,"y":frame.base.y.unwrap_or(0.0)+offset}),
                )
                .ok());
            }
        }
        let mut clone = match source {
            Source::Original(path) => self.clone_original(
                lookup::at(roots, path).unwrap(),
                &mut path.to_vec(),
                allocator,
                taken,
            )?,
            Source::Owned { .. } => walkers::deep_clone_with_allocator(original, allocator, taken)?,
        };
        walkers::translate_subtree(&mut clone, offset, offset);
        Ok(Some(clone))
    }
    pub(super) fn duplicate_source(
        &mut self,
        roots: &[PenNode],
        source: &Source,
        allocator: &mut dyn IdAllocator,
        taken: &mut HashSet<NodeId>,
        offset: f64,
    ) -> Result<Option<NodeId>, IdAllocError> {
        let Some(clone) = self.duplicate_payload(roots, source, allocator, taken, offset)? else {
            return Ok(None);
        };
        let Some(id) = NodeId::new_opt(clone.id_str()) else {
            return Ok(None);
        };
        match source {
            Source::Original(path) => self.insert_original_after(roots, path, clone),
            Source::Owned { root, path } => self.insert_owned_after(*root, path, clone),
        }
        Ok(Some(id))
    }
}
