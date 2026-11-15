//! Validation and the single undo boundary for host-computed Boolean paths.

use std::collections::HashSet;

use crate::{BooleanPathReplacement, EditorState, IdAllocError, IdAllocator, NodeId, PenNodeExt};
use jian_ops_schema::node::PenNode;

impl EditorState {
    /// Boolean replacement requires actual authored leaf shapes. Virtual
    /// instance children, ambiguous IDs, hidden/locked ancestry and container
    /// deletion are rejected before the native computation or any tree write.
    pub fn boolean_operands_are_editable(&self, ids: &[NodeId]) -> bool {
        if !(2..=512).contains(&ids.len()) || self.editor_ui.workspace.active {
            return false;
        }
        let wanted: HashSet<&str> = ids.iter().map(NodeId::as_str).collect();
        if wanted.len() != ids.len() || wanted.contains("") {
            return false;
        }
        let mut seen = HashSet::with_capacity(ids.len());
        let mut stack = self
            .active_children()
            .iter()
            .map(|n| (n, false))
            .collect::<Vec<_>>();
        while let Some((node, ancestor_blocked)) = stack.pop() {
            let base = node.base();
            let blocked =
                ancestor_blocked || base.locked == Some(true) || base.visible == Some(false);
            if wanted.contains(node.id_str()) {
                if blocked
                    || !seen.insert(node.id_str())
                    || node.children().is_some_and(|children| !children.is_empty())
                    || !matches!(
                        node,
                        PenNode::Frame(_)
                            | PenNode::Rectangle(_)
                            | PenNode::Ellipse(_)
                            | PenNode::Polygon(_)
                            | PenNode::Path(_)
                            | PenNode::Line(_)
                    )
                {
                    return false;
                }
            }
            if let Some(children) = node.children() {
                stack.extend(children.iter().map(|child| (child, blocked)));
            }
        }
        seen.len() == wanted.len()
    }

    pub(super) fn cmd_boolean(
        &mut self,
        value: BooleanPathReplacement,
        allocator: &mut dyn IdAllocator,
    ) -> Result<bool, IdAllocError> {
        if value.document_generation != self.document_generation()
            || value.document_revision != self.document_revision()
            || value.active_page_index != self.ui.active_page_index
            || value.source_ids != self.selection.set
            || value.d.len() > 8 * 1024 * 1024
            || !self.boolean_operands_are_editable(&value.source_ids)
        {
            return Ok(false);
        }
        let snapshot = self.snapshot_for_undo();
        let Some(result_id) = self.replace_paths_with_svg_with_allocator(
            &value.source_ids,
            value.d,
            value.bounds,
            allocator,
        )?
        else {
            return Ok(false);
        };
        self.selection.anchor = result_id.clone();
        self.selection.set = vec![result_id];
        self.push_undo(snapshot);
        Ok(true)
    }
}
