//! A lexical route proof: no mutable editor borrow escapes between first-ID
//! resolution, borrowed admission, exact capture and direct metadata write.
use super::*;
use crate::command_node_attrs::bulk::{self, Prepared};
use crate::PenNodeExt;
use jian_ops_schema::node::PenNode;
impl EditorState {
    /// Exact conservative catalog authority for a node-header-only writer.
    /// Definition roots and externally owned prototypes keep full checkpoints.
    /// Document-backed child metadata resolves through canonical Doc instead of
    /// the compatibility root's deliberately absent child payload.
    pub fn metadata_target_preserves_components(&self, node: &PenNode) -> bool {
        if self.editor_ui.workspace.active || self.instance_write_virtual_anchor.is_some() {
            return false;
        }
        !matches!(node,PenNode::Frame(frame) if frame.reusable==Some(true))
            && self.components.components.iter().all(|component| {
                component.id.as_str() != node.id_str()
                    && self.components.is_document_backed(&component.id)
            })
    }
    /// A request-local proof for the live atomic driver, consumed before any
    /// unsupported command runs. It owns no routes or catalog snapshots.
    pub(crate) fn metadata_command_preserves_components(&self, command: &EditorCommand) -> bool {
        if matches!(command, EditorCommand::Batch { .. }) {
            let mut pending = vec![(command, 0usize)];
            let mut count = 0usize;
            while let Some((member, depth)) = pending.pop() {
                count += 1;
                if count > 4096 || depth > 32 {
                    return false;
                }
                if let EditorCommand::Batch { commands } = member {
                    if commands.is_empty()
                        || commands.len() > 4096usize.saturating_sub(count + pending.len())
                    {
                        return false;
                    }
                    pending.extend(commands.iter().map(|member| (member, depth + 1)));
                } else if !self.metadata_command_preserves_components(member) {
                    return false;
                }
            }
            // These setters cannot allocate, change IDs/pages or turn a target
            // into a definition. Each exact leaf proof therefore remains valid
            // after the preceding metadata setters in this same atomic batch.
            return true;
        }
        if bulk::prepare_metadata(command).is_none() {
            return false;
        }
        target_plan::plan(self, command)
            .as_ref()
            .and_then(|specs| target_plan::metadata_target(self, specs))
            .is_some_and(|node| self.metadata_target_preserves_components(node))
    }
    /// Apply one supported metadata command with one first-ID resolution.
    /// `admit` sees borrowed input BEFORE any owned capture. None leaves the
    /// output slot empty and applies nothing, permitting ordinary fallback.
    /// The caller's initially-empty slot owns rollback even if apply unwinds.
    /// No route or proof survives this call, generation, or a topology edit.
    pub fn try_apply_captured_metadata(
        &mut self,
        command: EditorCommand,
        capture: &mut Option<CommandEditCapture>,
        admit: impl FnOnce(&Self, &PenNode) -> bool,
    ) -> Option<bool> {
        self.observe_command_id_sequence(&command);
        if capture.is_some() {
            return None;
        }
        let prepared = bulk::prepare_metadata(&command)?;
        let command_scope::Classification::Scoped(plan) = command_scope::classify(self, &command)
        else {
            return None;
        };
        if plan.header {
            return None;
        }
        let specs = target_plan::plan_prepared(
            self,
            &command,
            matches!(&prepared, Prepared::NumericPatch(_)),
        )?;
        let target = target_plan::metadata_target(self, &specs)?;
        if !admit(self, target) {
            return None;
        }
        let pending = if self.metadata_target_preserves_components(target) {
            PendingScopedSnapshot::capture_targets_preserving_components(self, plan.forests, specs)
        } else {
            PendingScopedSnapshot::capture_targets(self, plan.forests, plan.header, specs)
        }
        .ok()?;
        *capture = Some(CommandEditCapture {
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
        });
        Some(self.apply_command_edit_inner(
            command,
            capture.as_mut().unwrap(),
            |_| {},
            Some(prepared),
        ))
    }
}

impl EditorState {
    /// Internal inspector writers in this lexical callback change only this
    /// canonical node's metadata. Instance materialization keeps its old path.
    pub(crate) fn try_edit_header_with_history(
        &mut self,
        node_id: &crate::NodeId,
        write: impl FnOnce(&mut PenNode) -> bool,
    ) -> Option<bool> {
        self.try_edit_header_with_history_inner(node_id, false, write)
    }
    /// This lexical operation changes only a canonical Ref target. Even an
    /// owned external catalog cannot be changed through that header write.
    pub(crate) fn try_edit_ref_target_with_history(
        &mut self,
        node_id: &crate::NodeId,
        target: &str,
    ) -> Option<bool> {
        self.try_edit_header_with_history_inner(node_id, true, |node| {
            let PenNode::Ref(reference) = node else {
                return false;
            };
            if reference.target == target {
                return false;
            }
            reference.target.clear();
            reference.target.push_str(target);
            true
        })
    }
    fn try_edit_header_with_history_inner(
        &mut self,
        node_id: &crate::NodeId,
        ref_target: bool,
        write: impl FnOnce(&mut PenNode) -> bool,
    ) -> Option<bool> {
        if self.undo_capture_suspended
            || self.editor_ui.workspace.active
            || self.instance_write_virtual_anchor.is_some()
        {
            return None;
        }
        let command = EditorCommand::SetNodeName {
            node_id: node_id.clone(),
            name: String::new(),
        };
        let command_scope::Classification::Scoped(plan) = command_scope::classify(self, &command)
        else {
            return None;
        };
        if plan.header {
            return None;
        }
        let specs = target_plan::plan(self, &command)?;
        let target = target_plan::metadata_target(self, &specs)?;
        crate::history_snapshot::command_capture_node_bytes(
            target,
            true,
            crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE,
        )
        .ok()?;
        let spec = specs.first()?.clone();
        let catalog_stable = ref_target
            && matches!(target, PenNode::Ref(_))
            && !self.components.is_document_backed(node_id);
        let pending = if catalog_stable {
            PendingScopedSnapshot::capture_ref_target(self, plan.forests, specs)
        } else if self.metadata_target_preserves_components(target) {
            PendingScopedSnapshot::capture_targets_preserving_components(self, plan.forests, specs)
        } else {
            PendingScopedSnapshot::capture_targets(self, plan.forests, false, specs)
        }
        .ok()?;
        let roots = match spec.scope {
            crate::history::Scope::Root => &mut self.doc.children,
            crate::history::Scope::Page { index, .. } => {
                &mut self.doc.pages.as_mut().unwrap()[index].children
            }
        };
        let mut nodes = roots;
        let (last, parents) = spec.path.split_last().unwrap();
        for at in parents {
            nodes = nodes[*at]
                .children_mut()
                .expect("captured metadata ancestor");
        }
        let wrote = write(&mut nodes[*last]);
        let snapshot = pending
            .compact(self)
            .expect("metadata writer preserved captured topology");
        if snapshot.has_canonical_changes() {
            self.history_push_entry(HistoryEntry::Scoped(std::sync::Arc::new(snapshot)));
        }
        Some(wrote)
    }
}
