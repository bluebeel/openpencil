//! Opt-in, bounded MCP actor contexts. Documents and undo storage stay shared.
//!
//! A store belongs to one host/account. `agentId` is a client context identifier,
//! never an authentication credential. Calls without it retain legacy behavior.

use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use jian_ops_schema::node::PenNode;
use op_editor_core::{EditorState, HistoryActor, PenNodeExt, SelectionState};

use super::{tool_profile::McpAccessProfile, McpServeError};
#[path = "agent_context_selection.rs"]
mod selected;

#[path = "agent_context_error.rs"]
mod error;
pub(crate) use error::AgentContextError;

const UI_ALLOWANCE: usize = 64 * 1024;
const MAX_CLIPBOARD: usize = 4 * 1024 * 1024;
const MAX_SELECTION: usize = 1024;
const AGENT_BASE: usize = 304;
#[path = "agent_history_limits.rs"]
mod history_limits;

#[derive(Default)]
struct UiContext {
    selection: SelectionState,
    viewport: op_editor_core::viewport::Viewport,
    active_page: usize,
    active_page_id: Option<String>,
    tool: op_editor_core::tool::Tool,
}

impl UiContext {
    fn charge(&self) -> usize {
        256 + self.active_page_id.as_ref().map_or(0, String::capacity)
            + self.selection.set.capacity() * std::mem::size_of::<op_editor_core::NodeId>()
            + self
                .selection
                .set
                .iter()
                .map(|id| id.as_str().len())
                .sum::<usize>()
            + self.selection.anchor.as_str().len()
    }
}

#[derive(Default)]
struct AgentData {
    documents: HashMap<PathBuf, UiContext>,
    clipboard: Vec<PenNode>,
    clipboard_charge: usize,
}

impl AgentData {
    fn charge(&self) -> usize {
        AGENT_BASE
            + self.clipboard_charge
            + self
                .documents
                .iter()
                .map(|(key, ui)| key.as_os_str().len() + ui.charge())
                .sum::<usize>()
    }
}

#[path = "agent_context_execution.rs"]
mod execution;
use execution::AdmissionOwner;
pub(crate) use execution::{
    with_agent_context_for_commit, with_agent_context_permit, AgentDeliveryScope,
    AgentExecutionPermit,
};

struct Slot {
    history_actor: HistoryActor,
    admitted: Option<AdmissionOwner>,
    execution_pins: usize,
    execution_reservation: usize,
    admission_released: bool,
    data: Option<AgentData>,
    charge: usize,
}

#[derive(Default)]
struct Ledger {
    next_admission: u64,
    agents: HashMap<String, Slot>,
    charge: usize,
}

/// Retains UI records and clipboards, never an editor/document/history clone.
/// Capacity failures are explicit; contexts are never silently evicted.
pub(crate) struct AgentContexts {
    ledger: Mutex<Ledger>,
    max_agents: usize,
    max_documents_per_agent: usize,
    max_bytes: usize,
}

impl Default for AgentContexts {
    fn default() -> Self {
        Self::with_limits(4096, 32, 64 * 1024 * 1024)
    }
}

impl AgentContexts {
    fn with_limits(max_agents: usize, max_documents_per_agent: usize, max_bytes: usize) -> Self {
        Self {
            ledger: Mutex::new(Ledger::default()),
            max_agents,
            max_documents_per_agent,
            max_bytes,
        }
    }

    /// Claim the agent before waiting for a document lock. The token covers the
    /// whole request so concurrent calls using the same agentId fail explicitly.
    pub(crate) fn try_admit(
        &self,
        line: &str,
        profile: McpAccessProfile,
    ) -> Result<AgentAdmission<'_>, AgentContextError> {
        if super::sniff::is_control_message(line) {
            return Ok(AgentAdmission {
                store: self,
                agent: None,
                ticket: None,
            });
        }
        let Some(call) = op_mcp::parse_tool_call(line.trim()) else {
            return Ok(AgentAdmission {
                store: self,
                agent: None,
                ticket: None,
            });
        };
        let Some(agent) = call.arguments.get("agentId") else {
            return Ok(AgentAdmission {
                store: self,
                agent: None,
                ticket: None,
            });
        };
        if let Some(denied) = profile.refuse_call(&call.tool, &call.arguments) {
            return Err(AgentContextError::Denied {
                message: denied.message(&call.tool),
            });
        }
        validate_agent(agent)?;
        let mut ledger = self.ledger.lock().unwrap_or_else(|p| p.into_inner());
        let ticket = ledger
            .next_admission
            .checked_add(1)
            .ok_or(AgentContextError::AdmissionExhausted)?;
        let owner = AdmissionOwner {
            thread: std::thread::current().id(),
            ticket,
        };
        if let Some(slot) = ledger.agents.get_mut(agent) {
            if slot.admitted.is_some() || slot.data.is_none() {
                return Err(AgentContextError::Busy);
            }
            slot.admitted = Some(owner);
            slot.admission_released = false;
        } else {
            if ledger.agents.len() >= self.max_agents {
                return Err(AgentContextError::Capacity {
                    resource: "agents",
                    limit: self.max_agents,
                });
            }
            if ledger.charge + AGENT_BASE > self.max_bytes {
                return Err(AgentContextError::Capacity {
                    resource: "context bytes",
                    limit: self.max_bytes,
                });
            }
            let history_actor =
                HistoryActor::allocate().ok_or(AgentContextError::AdmissionExhausted)?;
            ledger.charge += AGENT_BASE;
            ledger.agents.insert(
                agent.clone(),
                Slot {
                    history_actor,
                    admitted: Some(owner),
                    execution_pins: 0,
                    execution_reservation: 0,
                    admission_released: false,
                    data: Some(AgentData::default()),
                    charge: AGENT_BASE,
                },
            );
        }
        ledger.next_admission = ticket;
        Ok(AgentAdmission {
            store: self,
            agent: Some(agent.clone()),
            ticket: Some(ticket),
        })
    }

    fn checkout_with_permit<'a>(
        &'a self,
        agent: &str,
        document: &Path,
        permit: Option<&AgentExecutionPermit>,
    ) -> Result<Lease<'a>, AgentContextError> {
        let mut ledger = self.ledger.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(slot) = ledger.agents.get(agent) {
            let allowed = match permit {
                Some(permit) => slot
                    .admitted
                    .is_some_and(|owner| permit.allows(self, agent, owner)),
                None => slot.admitted.is_none_or(|owner| {
                    !slot.admission_released && owner.thread == std::thread::current().id()
                }),
            };
            if slot.data.is_none() || !allowed {
                return Err(AgentContextError::Busy);
            }
            let data = slot.data.as_ref().unwrap();
            if !data.documents.contains_key(document)
                && data.documents.len() >= self.max_documents_per_agent
            {
                return Err(AgentContextError::Capacity {
                    resource: "documents per agent",
                    limit: self.max_documents_per_agent,
                });
            }
        } else if permit.is_some() {
            return Err(AgentContextError::Busy);
        } else if ledger.agents.len() >= self.max_agents {
            return Err(AgentContextError::Capacity {
                resource: "agents",
                limit: self.max_agents,
            });
        }
        let previous = ledger.agents.get(agent).map_or(0, |slot| slot.charge);
        let actual = ledger
            .agents
            .get(agent)
            .and_then(|slot| slot.data.as_ref())
            .map_or(AGENT_BASE, AgentData::charge);
        let pinned = ledger
            .agents
            .get(agent)
            .map_or(0, |slot| slot.execution_reservation);
        // A queued permit owns its original UI allowance through save/rollback;
        // unrelated documents cannot consume that reservation between stages.
        let reservation = execution::reservation::checkout(
            actual.max(AGENT_BASE),
            pinned,
            permit.is_some(),
            UI_ALLOWANCE,
            document.as_os_str().len(),
        )
        .ok_or(AgentContextError::Capacity {
            resource: "queued actor UI bytes",
            limit: pinned,
        })?;
        let total = ledger.charge - previous + reservation;
        if total > self.max_bytes {
            return Err(AgentContextError::Capacity {
                resource: "context bytes",
                limit: self.max_bytes,
            });
        }
        let history_actor = match ledger.agents.get(agent) {
            Some(slot) => slot.history_actor,
            None => HistoryActor::allocate().ok_or(AgentContextError::AdmissionExhausted)?,
        };
        ledger.charge = total;
        let slot = ledger.agents.entry(agent.into()).or_insert_with(|| Slot {
            history_actor,
            admitted: None,
            execution_pins: 0,
            execution_reservation: 0,
            admission_released: true,
            data: Some(AgentData::default()),
            charge: 0,
        });
        slot.charge = reservation;
        let data = slot.data.take().unwrap();
        Ok(Lease {
            store: self,
            agent: agent.into(),
            data: Some(data),
            reservation,
            history_actor,
        })
    }
}

pub(crate) struct AgentAdmission<'a> {
    store: &'a AgentContexts,
    agent: Option<String>,
    ticket: Option<u64>,
}
impl AgentAdmission<'_> {
    pub(crate) fn is_active(&self) -> bool {
        self.agent.is_some()
    }
}

impl Drop for AgentAdmission<'_> {
    fn drop(&mut self) {
        if let Some(agent) = &self.agent {
            let mut ledger = self.store.ledger.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(slot) = ledger.agents.get_mut(agent) {
                if slot
                    .admitted
                    .is_some_and(|owner| Some(owner.ticket) == self.ticket)
                {
                    slot.admission_released = true;
                    if slot.execution_pins == 0 {
                        slot.admitted = None;
                    }
                }
            }
        }
    }
}

fn validate_agent(agent: &str) -> Result<(), AgentContextError> {
    if agent.is_empty()
        || agent.len() > 96
        || !agent
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c))
    {
        Err(AgentContextError::InvalidAgent)
    } else {
        Ok(())
    }
}

struct Lease<'a> {
    store: &'a AgentContexts,
    agent: String,
    data: Option<AgentData>,
    reservation: usize,
    history_actor: HistoryActor,
}

impl Lease<'_> {
    fn reserve_clipboard(&mut self, charge: usize) -> Result<(), AgentContextError> {
        if charge > MAX_CLIPBOARD {
            return Err(AgentContextError::Capacity {
                resource: "clipboard bytes",
                limit: MAX_CLIPBOARD,
            });
        }
        let data = self.data.as_ref().unwrap();
        let required = self
            .reservation
            .max(self.reservation - data.clipboard_charge + charge);
        let mut ledger = self.store.ledger.lock().unwrap_or_else(|p| p.into_inner());
        if ledger.charge - self.reservation + required > self.store.max_bytes {
            return Err(AgentContextError::Capacity {
                resource: "context bytes",
                limit: self.store.max_bytes,
            });
        }
        ledger.charge = ledger.charge - self.reservation + required;
        ledger.agents.get_mut(&self.agent).unwrap().charge = required;
        self.reservation = required;
        Ok(())
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        let Some(data) = self.data.take() else { return };
        let actual = data.charge();
        let mut ledger = self.store.ledger.lock().unwrap_or_else(|p| p.into_inner());
        let pinned = ledger
            .agents
            .get(&self.agent)
            .map_or(0, |slot| slot.execution_reservation);
        let charge = actual.max(pinned);
        ledger.charge = ledger.charge - self.reservation + charge;
        let slot = ledger.agents.get_mut(&self.agent).unwrap();
        slot.charge = charge;
        slot.data = Some(data);
    }
}

/// The local desktop/stdio transports have one process-wide authenticated host.
/// Multi-tenant and serve-web callers supply their own host/account-owned store.
pub(crate) fn default_agent_contexts() -> &'static AgentContexts {
    static CONTEXTS: OnceLock<AgentContexts> = OnceLock::new();
    CONTEXTS.get_or_init(AgentContexts::default)
}

fn refusal(call: &op_mcp::ToolCall, message: String) -> Result<Option<String>, McpServeError> {
    Ok(Some(op_mcp::tool_response_to_json(
        &op_mcp::ToolResponse::Err {
            id: call.id.clone(),
            code: op_mcp::ToolErrorCode::ToolFailed,
            message,
        },
    )))
}

/// Scope registry construction AND command application to a client's UI state.
/// The caller must already hold the canonical document authority's writer lock.
/// The store lock is held only for admission/accounting, never during dispatch.
/// Panics become normal tool errors after live UI restoration; applied document
/// writes and the shared undo history are intentionally not rolled back.
pub(crate) fn with_agent_context(
    contexts: &AgentContexts,
    document: &Path,
    state: &mut EditorState,
    line: &str,
    profile: McpAccessProfile,
    action: impl FnOnce(&mut EditorState) -> Result<Option<String>, McpServeError>,
) -> Result<Option<String>, McpServeError> {
    if let Some(permit) = execution::delivery_permit(contexts, document, line) {
        return with_agent_context_permit(
            contexts, document, state, line, profile, &permit, action,
        );
    }
    with_agent_context_impl(
        contexts,
        document,
        state,
        line,
        profile,
        None,
        Phase::Request,
        action,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Request,
    Commit,
}

fn with_agent_context_impl(
    contexts: &AgentContexts,
    document: &Path,
    state: &mut EditorState,
    line: &str,
    profile: McpAccessProfile,
    permit: Option<&AgentExecutionPermit>,
    phase: Phase,
    action: impl FnOnce(&mut EditorState) -> Result<Option<String>, McpServeError>,
) -> Result<Option<String>, McpServeError> {
    if super::sniff::is_control_message(line) {
        return action(state);
    }
    if phase == Phase::Request {
        let limits = history_limits::configured()
            .map_err(|error| McpServeError::Validation(error.to_string()))?;
        state
            .history
            .configure_limits(limits)
            .map_err(|error| McpServeError::Validation(error.to_string()))?;
    }
    let Some(call) = op_mcp::parse_tool_call(line.trim()) else {
        return action(state);
    };
    let Some(agent) = call.arguments.get("agentId") else {
        return action(state);
    };
    if let Some(denied) = profile.refuse_call(&call.tool, &call.arguments) {
        return refusal(&call, denied.message(&call.tool));
    }
    if let Err(error) = validate_agent(agent) {
        return refusal(&call, error.to_string());
    }
    let lease = match contexts.checkout_with_permit(agent, document, permit) {
        Ok(lease) => lease,
        Err(error) => return refusal(&call, error.to_string()),
    };
    let mut scope = Scope::enter(lease, document, state, phase);
    if phase == Phase::Request && matches!(call.tool.as_str(), "undo" | "redo") {
        if let Err(error) = scope.state.validate_actor_history(call.tool == "redo") {
            return refusal(&call, error.to_string());
        }
    }
    // Both production clipboard writers copy this exact selected forest. Charge
    // before registry/application, without cloning it or making a JSON buffer.
    if phase == Phase::Request && matches!(call.tool.as_str(), "copy_selected" | "cut_selected") {
        let result = selected_clipboard_charge(scope.state)
            .and_then(|bytes| scope.lease.reserve_clipboard(bytes));
        if let Err(error) = result {
            return refusal(&call, error.to_string());
        }
    }
    if phase == Phase::Request && super::atomic_tools::requests_clipboard_reservation(line) {
        if let Err(error) = scope.lease.reserve_clipboard(MAX_CLIPBOARD) {
            return refusal(&call, error.to_string());
        }
    }
    let mut outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| action(scope.state)));
    if phase == Phase::Request {
        if let Some(error) = scope.state.take_actor_history_error() {
            if matches!(&outcome, Ok(Ok(_))) {
                outcome = Ok(refusal(&call, error.to_string()));
            }
        }
    }
    let restored = scope.finish();
    if let Err(error) = restored {
        if let Ok(Ok(Some(response))) = &outcome {
            if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(response) {
                json["result"]["_meta"]["openpencil/agentContextWarning"] =
                    error.to_string().into();
                return Ok(Some(json.to_string()));
            }
        }
        return refusal(&call, error.to_string());
    }
    match outcome {
        Ok(result) => result,
        Err(_) => refusal(&call, "agent-context-call-failed: tool panicked; live UI restored, document/history remain shared".into()),
    }
}

struct Scope<'a, 's> {
    lease: Lease<'a>,
    state: &'s mut EditorState,
    document: PathBuf,
    live: UiContext,
    live_history_actor: Option<HistoryActor>,
    live_clipboard: Vec<PenNode>,
    // Protect interactive text/property drafts too. Variable/theme caches remain
    // shared and are moved back after the call, including canonical edits.
    live_ui: Option<op_editor_core::ui_draft::UiDraftState>,
    live_image_panel: op_editor_core::image_panel_state::ImagePanelState,
    live_crop_editing: Option<op_editor_core::NodeId>,
    clipboard_identity: (*const PenNode, usize),
    finished: bool,
}

impl<'a, 's> Scope<'a, 's> {
    fn enter(
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
        let mut agent_ui = data.documents.remove(document).unwrap_or_default();
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
        selected::prune(&mut agent_ui.selection, state.active_children());
        agent_ui.active_page = live_ui.active_page_index;
        std::mem::swap(&mut state.selection, &mut agent_ui.selection);
        std::mem::swap(&mut state.viewport, &mut agent_ui.viewport);
        std::mem::swap(&mut state.tool, &mut agent_ui.tool);
        let live_clipboard =
            std::mem::replace(&mut state.clipboard, std::mem::take(&mut data.clipboard));
        let clipboard_identity = (state.clipboard.as_ptr(), state.clipboard.len());
        let live_image_panel = std::mem::take(&mut state.editor_ui.image_panel);
        let live_crop_editing = state.editor_ui.image_crop_editing.take();
        Self {
            lease,
            state,
            document: document.into(),
            live: agent_ui,
            live_history_actor,
            live_clipboard,
            live_ui: Some(live_ui),
            live_image_panel,
            live_crop_editing,
            clipboard_identity,
            finished: false,
        }
    }

    fn finish(&mut self) -> Result<(), AgentContextError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        self.state.set_history_actor(self.live_history_actor);
        let data = self.lease.data.as_mut().unwrap();
        let clipboard_changed =
            self.clipboard_identity != (self.state.clipboard.as_ptr(), self.state.clipboard.len());
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
        selected::prune(&mut selection, self.state.active_children());
        self.state.selection = selection;
        data.clipboard = std::mem::replace(
            &mut self.state.clipboard,
            std::mem::take(&mut self.live_clipboard),
        );
        let mut error = None;
        if clipboard_changed {
            match clipboard_charge(data.clipboard.iter(), data.clipboard.capacity()) {
                Ok(bytes) => data.clipboard_charge = bytes,
                Err(message) => {
                    data.clipboard = Vec::new();
                    data.clipboard_charge = 0;
                    error = Some(message);
                }
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

struct ByteCounter {
    bytes: usize,
}
impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.saturating_add(bytes.len());
        if self.bytes > MAX_CLIPBOARD / 4 {
            return Err(std::io::ErrorKind::OutOfMemory.into());
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn clipboard_charge<'a>(
    nodes: impl Iterator<Item = &'a PenNode>,
    root_capacity: usize,
) -> Result<usize, AgentContextError> {
    fn node_bytes(node: &PenNode) -> usize {
        node.children().map_or(0, |nodes| {
            nodes.capacity() * std::mem::size_of::<PenNode>() * 2
                + nodes.iter().map(node_bytes).sum::<usize>()
        })
    }
    let mut serialized = ByteCounter { bytes: 0 };
    let mut storage = root_capacity * std::mem::size_of::<PenNode>() * 2;
    for node in nodes {
        serde_json::to_writer(&mut serialized, node)
            .map_err(|_| AgentContextError::ClipboardSerialization)?;
        storage = storage.saturating_add(node_bytes(node));
    }
    let bytes = serialized.bytes.saturating_mul(4).saturating_add(storage);
    if bytes > MAX_CLIPBOARD {
        Err(AgentContextError::Capacity {
            resource: "clipboard bytes",
            limit: MAX_CLIPBOARD,
        })
    } else {
        Ok(bytes)
    }
}

/// Atomic preparation rejects an oversized clipboard before any live command.
pub(super) fn validate_atomic_clipboard(state: &EditorState) -> Result<(), AgentContextError> {
    clipboard_charge(state.clipboard.iter(), state.clipboard.capacity()).map(|_| ())
}

pub(super) fn validate_atomic_ui(state: &EditorState) -> Result<(), AgentContextError> {
    if state.selection.len() > MAX_SELECTION {
        return Err(AgentContextError::Capacity {
            resource: "atomic selection entries",
            limit: MAX_SELECTION,
        });
    }
    let ui = UiContext {
        selection: state.selection.clone(),
        viewport: state.viewport,
        active_page: state.ui.active_page_index,
        active_page_id: current_page_id(state),
        tool: state.tool,
    };
    if ui.charge() > UI_ALLOWANCE / 2 {
        return Err(AgentContextError::Capacity {
            resource: "atomic UI bytes",
            limit: UI_ALLOWANCE / 2,
        });
    }
    Ok(())
}

fn selected_clipboard_charge(state: &EditorState) -> Result<usize, AgentContextError> {
    clipboard_charge(
        selected::roots(&state.selection, state.active_children()).into_iter(),
        state.selection.set.len(),
    )
}

#[cfg(test)]
#[path = "agent_context_tests.rs"]
mod tests;

pub(crate) fn current_page_id(state: &EditorState) -> Option<String> {
    state
        .doc
        .pages
        .as_ref()?
        .get(state.ui.active_page_index)
        .map(|page| page.id.clone())
}
fn resolve_page(state: &EditorState, hint: usize, id: Option<&str>) -> usize {
    if let (Some(id), Some(pages)) = (id, state.doc.pages.as_ref()) {
        if pages.get(hint).is_some_and(|page| page.id == id) {
            return hint;
        }
        let mut found = pages.iter().enumerate().filter(|(_, page)| page.id == id);
        if let Some((index, _)) = found.next() {
            if found.next().is_none() {
                return index;
            }
        }
    }
    hint.min(state.page_count().saturating_sub(1))
}
pub(crate) fn restore_active_page(state: &mut EditorState, hint: usize, id: Option<&str>) {
    state.ui.active_page_index = resolve_page(state, hint, id);
}
