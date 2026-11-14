//! Actor state outside document history that an atomic operation must restore.

use crate::{EditorCommand, EditorState};
use jian_ops_schema::node::PenNode;

#[derive(Debug)]
pub(crate) struct BatchActorSnapshot {
    ui: crate::ui_draft::UiDraftState,
    tool: crate::tool::Tool,
    viewport: crate::viewport::Viewport,
    image_panel: crate::image_panel_state::ImagePanelState,
    crop_editing: Option<crate::NodeId>,
    clipboard: Option<Vec<PenNode>>,
}

impl BatchActorSnapshot {
    pub(crate) fn for_command(state: &EditorState, command: &EditorCommand) -> Option<Self> {
        match command {
            EditorCommand::Batch { commands } => Some(Self::capture(state, commands)),
            _ => None,
        }
    }

    pub(crate) fn capture(state: &EditorState, commands: &[EditorCommand]) -> Self {
        Self {
            ui: state.ui.clone(),
            tool: state.tool,
            viewport: state.viewport,
            image_panel: state.editor_ui.image_panel.clone(),
            crop_editing: state.editor_ui.image_crop_editing.clone(),
            // Paste only reads this buffer. Clone it only if a writer can
            // replace it, including when a later durable save is rejected.
            clipboard: commands
                .iter()
                .any(EditorCommand::changes_clipboard)
                .then(|| state.clipboard.clone()),
        }
    }

    pub(crate) fn restore(self, state: &mut EditorState) {
        state.ui = self.ui;
        state.tool = self.tool;
        state.viewport = self.viewport;
        state.editor_ui.image_panel = self.image_panel;
        state.editor_ui.image_crop_editing = self.crop_editing;
        if let Some(clipboard) = self.clipboard {
            state.clipboard = clipboard;
        }
    }
}

impl EditorCommand {
    /// Whether this command can replace the actor's editor clipboard.
    pub fn changes_clipboard(&self) -> bool {
        match self {
            Self::CopySelected | Self::CutSelected => true,
            Self::Batch { commands } => commands.iter().any(Self::changes_clipboard),
            _ => false,
        }
    }
}
