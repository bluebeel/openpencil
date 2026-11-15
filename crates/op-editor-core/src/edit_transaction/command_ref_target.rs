//! Canonical Ref retargeting owns one exact header, never the independent catalog.
//! Routes are fresh unless the enclosing locked command authority owns its index.
use super::*;
use crate::{
    history::{Scope, TargetSpec},
    NodeId, PenNodeExt,
};
use jian_ops_schema::node::PenNode;

impl EditorState {
    /// The component mutators use preorder DFS, unlike CRUD's sibling-first
    /// duplicate-ID rule. A cached miss is authoritative only inside the real
    /// lexical command authority; ordinary mutable Doc callers walk afresh.
    pub(crate) fn component_edit_target_spec(
        &self,
        id: &NodeId,
        active: usize,
    ) -> Option<TargetSpec> {
        if !id.is_real() {
            return None;
        }
        let scope = command_scope::scope(self, active);
        let path = match self.command_lookup_path(
            &scope,
            id,
            crate::command_id_sequence::LookupOrder::Preorder,
        ) {
            Some(path) => path?,
            None => {
                let nodes = match &scope {
                    Scope::Root => self.doc.children.as_slice(),
                    Scope::Page { index, .. } => {
                        self.doc.pages.as_ref()?.get(*index)?.children.as_slice()
                    }
                };
                preorder(nodes, id.as_str(), &mut Vec::new())?.into_boxed_slice()
            }
        };
        Some(TargetSpec {
            scope,
            path,
            subtree: false,
            splice: None,
            topology: None,
        })
    }

    pub(super) fn ref_target_command_spec(
        &self,
        command: &EditorCommand,
        active: usize,
    ) -> Option<TargetSpec> {
        let EditorCommand::SetInstanceComponent {
            node_id,
            component_id,
        } = command
        else {
            return None;
        };
        if self.editor_ui.workspace.active
            || self.instance_write_virtual_anchor.is_some()
            || !component_id.is_real()
            || self.components.is_document_backed(node_id)
        {
            return None;
        }
        self.components.find_by_id(component_id)?;
        let spec = self.component_edit_target_spec(node_id, active)?;
        let specs = std::slice::from_ref(&spec);
        matches!(target_plan::metadata_target(self, specs)?, PenNode::Ref(_)).then_some(spec)
    }

    /// Bounds the real old/new header copies and path before cloning anything.
    /// A referenced catalog prototype is borrowed, never materialized or sized.
    pub fn forecast_ref_target_command_edit(
        &self,
        command: &EditorCommand,
        limit: usize,
    ) -> Option<usize> {
        if self.undo_capture_suspended {
            return None;
        }
        let spec = self.ref_target_command_spec(command, self.ui.active_page_index)?;
        let node = target_plan::metadata_target(self, std::slice::from_ref(&spec))?;
        let EditorCommand::SetInstanceComponent { component_id, .. } = command else {
            return None;
        };
        let bytes = crate::history_snapshot::command_capture_node_bytes(node, true, limit).ok()?;
        let bound = bytes
            .checked_mul(8)?
            .checked_add(component_id.as_str().len().checked_mul(8)?)?
            .checked_add(
                spec.path
                    .len()
                    .checked_mul(8 * std::mem::size_of::<usize>())?,
            )?
            .checked_add(128 * 1024)?;
        (bound <= limit).then_some(bound)
    }

    pub(super) fn prepare_ref_target_command_edit(
        &self,
        command: &EditorCommand,
    ) -> Option<CommandEditCapture> {
        self.forecast_ref_target_command_edit(
            command,
            crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE,
        )?;
        let spec = self.ref_target_command_spec(command, self.ui.active_page_index)?;
        let pending =
            PendingScopedSnapshot::capture_ref_target(self, vec![spec.scope.clone()], vec![spec])
                .ok()?;
        Some(CommandEditCapture {
            rollback_clipboard: None,
            batch_actor: None,
            kind: CaptureKind::Scoped(ScopedCapture {
                targeted: true,
                generation: self.document_generation,
                revision: self.revision,
                pending: Some(pending),
                completed: None,
                history: self.history.clone(),
                active_theme: self.ui.variables.active_theme.clone(),
            }),
        })
    }

    /// Admission, exact capture and the existing real Core mutator share one
    /// exclusive borrow. None applies nothing; the caller may isolate/fallback.
    pub fn try_apply_captured_ref_target(
        &mut self,
        command: EditorCommand,
        capture: &mut Option<CommandEditCapture>,
        admit: impl FnOnce(&Self, usize) -> bool,
    ) -> Option<bool> {
        self.observe_command_id_sequence(&command);
        if capture.is_some() {
            return None;
        }
        let bytes = self.forecast_ref_target_command_edit(
            &command,
            crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE,
        )?;
        if !admit(self, bytes) {
            return None;
        }
        *capture = Some(self.prepare_ref_target_command_edit(&command)?);
        Some(self.apply_command_edit_without_document_repairs(command, capture.as_mut().unwrap()))
    }
}
fn preorder(nodes: &[PenNode], id: &str, path: &mut Vec<usize>) -> Option<Vec<usize>> {
    for (index, node) in nodes.iter().enumerate() {
        path.push(index);
        if node.id_str() == id {
            return Some(path.clone());
        }
        if let Some(found) = node
            .children()
            .and_then(|children| preorder(children, id, path))
        {
            return Some(found);
        }
        path.pop();
    }
    None
}
