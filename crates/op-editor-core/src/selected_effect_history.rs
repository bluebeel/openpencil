//! Exact canonical header history for inspector winding/effect edits.
use crate::EditorState;
use jian_ops_schema::node::PenNode;

impl EditorState {
    /// Set the selected Path's winding rule and record one undo step.
    pub fn set_selected_fill_rule(
        &mut self,
        fill_rule: jian_ops_schema::node::path::PathFillRule,
    ) -> bool {
        let sel = self.selection.anchor.clone();
        if !sel.is_real() || !self.is_editable(&sel) {
            return false;
        }
        if let Some(wrote) = self.try_edit_header_with_history(&sel, |node| {
            let PenNode::Path(path) = node else {
                return false;
            };
            path.fill_rule = Some(fill_rule.clone());
            true
        }) {
            return wrote;
        }
        let before = self.snapshot_for_history();
        let instance_scope = self.begin_instance_write_for_anchor();
        let wrote = self.cmd_set_path_fill_rule(&sel, fill_rule);
        if let Some(scope) = instance_scope {
            self.finish_instance_write(scope);
        }
        if wrote && self.snapshot_for_history() != before {
            self.history_push_past(before);
        }
        wrote
    }

    /// Remove one selected-node effect and record one undo step.
    pub fn remove_selected_effect(&mut self, index: usize) -> bool {
        let sel = self.selection.anchor.clone();
        if !sel.is_real() || !self.is_editable(&sel) {
            return false;
        }
        if let Some(wrote) = self.try_edit_header_with_history(&sel, |node| {
            crate::command_node_attrs::borrowed::remove_node_effect(node, &sel, index as u32)
        }) {
            return wrote;
        }
        let before = self.snapshot_for_history();
        let instance_scope = self.begin_instance_write_for_anchor();
        let wrote = self.cmd_remove_node_effect(&sel, index as u32);
        if let Some(scope) = instance_scope {
            self.finish_instance_write(scope);
        }
        if wrote && self.snapshot_for_history() != before {
            self.history_push_past(before);
        }
        wrote
    }

    /// Write the shared optional effect visibility flag and record one undo step.
    pub fn set_selected_effect_visible(&mut self, index: usize, visible: bool) -> bool {
        let sel = self.selection.anchor.clone();
        if !sel.is_real() || !self.is_editable(&sel) {
            return false;
        }
        if let Some(wrote) = self.try_edit_header_with_history(&sel, |node| {
            let Some(effect) =
                crate::fills::node_effects_opt_mut(node).and_then(|effects| effects.get_mut(index))
            else {
                return false;
            };
            match effect {
                jian_ops_schema::style::PenEffect::Shadow(body) => body.visible = Some(visible),
                jian_ops_schema::style::PenEffect::Blur(body)
                | jian_ops_schema::style::PenEffect::BackgroundBlur(body) => {
                    body.visible = Some(visible)
                }
            }
            true
        }) {
            return wrote;
        }
        let before = self.snapshot_for_history();
        let instance_scope = self.begin_instance_write_for_anchor();
        let wrote = self.cmd_set_effect_visible(&sel, index, visible);
        if let Some(scope) = instance_scope {
            self.finish_instance_write(scope);
        }
        if wrote && self.snapshot_for_history() != before {
            self.history_push_past(before);
        }
        wrote
    }
}
