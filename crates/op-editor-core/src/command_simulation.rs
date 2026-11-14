//! Disposable command planning state; never install this as a real editor.
use crate::{EditorState, History};

impl EditorState {
    /// Clone every editor field for an independent disposable simulation, while
    /// excluding undo history that command planning never consumes. Document
    /// commands retain their normal validation, revision and ID rules; atomic
    /// rollback checkpoints remain active. This is not a real-edit boundary.
    pub fn clone_for_command_simulation(&self) -> Self {
        let mut simulation = self.clone();
        simulation.history = History::new();
        simulation.undo_capture_suspended = true;
        simulation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_support::sample, EditorCommand, NodeId};

    #[test]
    fn command_simulation_is_independent_and_retains_no_undo_history() {
        let mut source = sample();
        assert!(source.apply_undoable(EditorCommand::NudgeSelected { dx: 3, dy: 0 }));
        let document = serde_json::to_vec(&source.doc).unwrap();
        let history = source.history.clone();
        let revision = source.document_revision();
        let mut simulation = source.clone_for_command_simulation();
        assert!(simulation.history.past.is_empty() && simulation.history.future.is_empty());
        for _ in 0..150 {
            assert!(simulation.apply(EditorCommand::NudgeSelected { dx: 1, dy: 0 }));
        }
        assert!(simulation.history.past.is_empty() && simulation.history.future.is_empty());
        assert_ne!(serde_json::to_vec(&simulation.doc).unwrap(), document);
        assert_eq!(serde_json::to_vec(&source.doc).unwrap(), document);
        assert_eq!(source.history.past, history.past);
        assert_eq!(source.history.future, history.future);
        assert_eq!(source.document_revision(), revision);
    }

    #[test]
    fn command_simulation_preserves_allocation_revisions_and_atomic_rollback() {
        let source = sample();
        let mut ordinary = source.clone();
        ordinary.history = History::new();
        let mut simulation = source.clone_for_command_simulation();
        let insert = || EditorCommand::InsertNode {
            kind: "rect".into(),
            name: "Simulation".into(),
            x: 0,
            y: 0,
            width: 5,
            height: 5,
            fill_hex: None,
            target_parent: NodeId::NONE,
            page_id: None,
        };
        assert!(ordinary.apply(insert()));
        assert!(simulation.apply(insert()));
        assert_eq!(
            serde_json::to_vec(&simulation.doc).unwrap(),
            serde_json::to_vec(&ordinary.doc).unwrap()
        );
        assert_eq!(simulation.revision, ordinary.revision);
        let before = serde_json::to_vec(&simulation.doc).unwrap();
        let failure = || EditorCommand::Batch {
            commands: vec![
                insert(),
                EditorCommand::DeleteNode {
                    node_id: NodeId::new("missing"),
                    page_id: None,
                },
            ],
        };
        assert!(!ordinary.apply(failure()));
        assert!(!simulation.apply(failure()));
        assert_eq!(serde_json::to_vec(&simulation.doc).unwrap(), before);
        assert_eq!(serde_json::to_vec(&ordinary.doc).unwrap(), before);
        assert_eq!(simulation.revision, ordinary.revision);
        assert!(simulation.undo_capture_suspended);
        assert!(simulation.history.past.is_empty() && simulation.history.future.is_empty());
        assert!(ordinary.apply(insert()));
        assert!(simulation.apply(insert()));
        assert_eq!(
            serde_json::to_vec(&simulation.doc).unwrap(),
            serde_json::to_vec(&ordinary.doc).unwrap()
        );
    }
}
