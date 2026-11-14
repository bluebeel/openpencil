//! A lexical route proof: no mutable editor borrow escapes between first-ID
//! resolution, borrowed admission, exact capture and direct metadata write.
use super::*;
use crate::command_node_attrs::bulk::{self, Prepared};
use crate::PenNodeExt;
use jian_ops_schema::node::PenNode;
impl EditorState {
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
        if capture.is_some() {
            return None;
        }
        let prepared = bulk::prepare(&command)?;
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
        let pending =
            PendingScopedSnapshot::capture_targets(self, plan.forests, plan.header, specs).ok()?;
        *capture = Some(CommandEditCapture {
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
        let pending =
            PendingScopedSnapshot::capture_targets(self, plan.forests, false, specs).ok()?;
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
