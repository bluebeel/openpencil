use super::*;

pub(super) struct Scope<'a, 's> {
    pub(super) lease: Lease<'a>,
    pub(super) state: &'s mut EditorState,
    pub(super) document: PathBuf,
    pub(super) live: UiContext,
    pub(super) live_history_actor: Option<HistoryActor>,
    pub(super) live_clipboard: Vec<PenNode>,
    // Protect interactive text/property drafts too. Variable/theme caches remain
    // shared and are moved back after the call, including canonical edits.
    pub(super) live_ui: Option<op_editor_core::ui_draft::UiDraftState>,
    pub(super) live_image_panel: op_editor_core::image_panel_state::ImagePanelState,
    pub(super) live_crop_editing: Option<op_editor_core::NodeId>,
    pub(super) finished: bool,
}

impl<'a, 's> Scope<'a, 's> {
    pub(super) fn enter(
        mut lease: Lease<'a>,
        document: &Path,
        state: &'s mut EditorState,
        phase: Phase,
    ) -> Self {
        let live_history_actor = state.set_history_actor(Some(lease.history_actor));
        if phase == Phase::Request {
            state.take_actor_history_error();
        }
        let data = lease.data.as_mut().unwrap();
        let (document, mut agent_ui) = data
            .documents
            .remove_entry(document)
            .unwrap_or_else(|| (document.to_owned(), UiContext::default()));
        let mut live_ui = std::mem::take(&mut state.ui);
        state.ui.variables = std::mem::take(&mut live_ui.variables);
        state.ui.active_page_index = resolve_page(
            state,
            agent_ui.active_page,
            agent_ui.active_page_id.as_deref(),
        );
        agent_ui.active_page_id = state
            .doc
            .pages
            .as_ref()
            .and_then(|pages| pages.get(live_ui.active_page_index))
            .map(|page| page.id.clone());
        selected::prune(&mut agent_ui.selection, state);
        agent_ui.active_page = live_ui.active_page_index;
        std::mem::swap(&mut state.selection, &mut agent_ui.selection);
        std::mem::swap(&mut state.viewport, &mut agent_ui.viewport);
        std::mem::swap(&mut state.tool, &mut agent_ui.tool);
        let live_clipboard =
            std::mem::replace(&mut state.clipboard, std::mem::take(&mut data.clipboard));
        let live_image_panel = std::mem::take(&mut state.editor_ui.image_panel);
        let live_crop_editing = state.editor_ui.image_crop_editing.take();
        Self {
            lease,
            state,
            document,
            live: agent_ui,
            live_history_actor,
            live_clipboard,
            live_ui: Some(live_ui),
            live_image_panel,
            live_crop_editing,
            finished: false,
        }
    }

    pub(super) fn finish(&mut self) -> Result<(), AgentContextError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        self.state.set_history_actor(self.live_history_actor);
        let data = self.lease.data.as_mut().unwrap();
        std::mem::swap(&mut self.state.selection, &mut self.live.selection);
        std::mem::swap(&mut self.state.viewport, &mut self.live.viewport);
        std::mem::swap(&mut self.state.tool, &mut self.live.tool);
        let human_page_id = self.live.active_page_id.take();
        self.live.active_page = self.state.ui.active_page_index;
        self.live.active_page_id = current_page_id(self.state);
        let mut live_ui = self.live_ui.take().unwrap();
        live_ui.variables = std::mem::take(&mut self.state.ui.variables);
        self.state.ui = live_ui;
        self.state.editor_ui.image_panel = std::mem::take(&mut self.live_image_panel);
        self.state.editor_ui.image_crop_editing = self.live_crop_editing.take();
        self.state.ui.active_page_index = resolve_page(
            self.state,
            self.state.ui.active_page_index,
            human_page_id.as_deref(),
        );
        let mut selection = std::mem::take(&mut self.state.selection);
        selected::prune(&mut selection, self.state);
        self.state.selection = selection;
        data.clipboard = std::mem::replace(
            &mut self.state.clipboard,
            std::mem::take(&mut self.live_clipboard),
        );
        let mut error = None;
        // Vec address/length does not prove its public mutable node payload
        // stayed unchanged. Account in-place/custom edits too. Empty clipboards
        // remain O(1); no document or retained JSON buffer is materialized.
        match clipboard_charge(data.clipboard.iter(), data.clipboard.capacity()) {
            Ok(bytes) => data.clipboard_charge = bytes,
            Err(message) => {
                data.clipboard = Vec::new();
                data.clipboard_charge = 0;
                error = Some(message);
            }
        }
        if self
            .live
            .active_page_id
            .as_ref()
            .is_some_and(|id| id.capacity() > UI_ALLOWANCE / 4)
        {
            // Canonical page data is untouched. Refuse an unretainable actor
            // preference rather than keeping bytes outside the UI reservation.
            self.live.active_page_id = None;
            error = Some(AgentContextError::Capacity {
                resource: "page identity bytes",
                limit: UI_ALLOWANCE / 4,
            });
        }
        if self.live.selection.len() <= MAX_SELECTION
            && self.live.charge() > UI_ALLOWANCE / 2
            && self.live.selection.set.capacity() > self.live.selection.set.len()
        {
            // A previous larger selection can leave spare Vec capacity. Reclaim
            // it before discarding otherwise retainable logical UI state.
            self.live.selection.set.shrink_to_fit();
        }
        if self.live.selection.len() > MAX_SELECTION || self.live.charge() > UI_ALLOWANCE / 2 {
            self.live.selection = SelectionState::empty();
            error = Some(AgentContextError::OversizedSelection {
                limit: MAX_SELECTION,
            });
        }
        data.documents.insert(
            std::mem::take(&mut self.document),
            std::mem::take(&mut self.live),
        );
        // The allowance admits normal UI changes; unexpected custom clipboard
        // writers cannot leave unaccounted data retained after a call.
        if data.charge() > self.lease.reservation {
            data.clipboard = Vec::new();
            data.clipboard_charge = 0;
            error = Some(AgentContextError::UnaccountedClipboard);
        }
        error.map_or(Ok(()), Err)
    }
}

impl Drop for Scope<'_, '_> {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}
