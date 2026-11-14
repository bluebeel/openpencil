//! A lexical route proof: no mutable editor borrow escapes between first-ID
//! resolution, borrowed admission, exact capture and direct metadata write.
use super::*;
use crate::command_node_attrs::bulk::{self, Prepared};
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
