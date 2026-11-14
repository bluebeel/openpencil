//! Opaque original admission retained through commit and reply delivery.
//! Shared pins let owned async responses release request workers safely.
use super::*;
#[path = "agent_context_execution_identity.rs"]
mod identity;
#[path = "agent_context_execution_reservation.rs"]
pub(super) mod reservation;
pub(super) use identity::AdmissionOwner;

#[derive(Clone)]
pub(crate) struct AgentExecutionPermit {
    inner: std::sync::Arc<ExecutionPin>,
}
pub(crate) struct ExecutionPin {
    identity: identity::Identity,
    store: std::sync::Arc<AgentContexts>,
    document: PathBuf,
}

impl AgentAdmission<'_> {
    pub(crate) fn execution_permit(
        &self,
        store: &std::sync::Arc<AgentContexts>,
        document: &Path,
    ) -> Option<AgentExecutionPermit> {
        if !std::ptr::eq(self.store, store.as_ref()) {
            return None;
        }
        let agent = self.agent.as_ref()?;
        let mut ledger = self.store.ledger.lock().unwrap_or_else(|p| p.into_inner());
        let slot = ledger.agents.get(agent)?;
        let owner = slot.admitted?;
        if Some(owner.ticket) != self.ticket || slot.execution_pins != 0 {
            return None;
        }
        let (reservation, total) = reservation::pin(
            slot.data.as_ref()?.charge().max(AGENT_BASE),
            slot.charge,
            ledger.charge,
            UI_ALLOWANCE,
            document.as_os_str().len(),
            self.store.max_bytes,
        )?;
        let slot = ledger.agents.get_mut(agent)?;
        slot.execution_pins = 1;
        slot.execution_reservation = reservation;
        slot.charge = reservation;
        ledger.charge = total;
        Some(AgentExecutionPermit {
            inner: std::sync::Arc::new(ExecutionPin {
                store: std::sync::Arc::clone(store),
                document: document.to_owned(),
                identity: identity::Identity::new(
                    self.store as *const AgentContexts as usize,
                    agent.clone(),
                    owner,
                ),
            }),
        })
    }
}

impl Drop for ExecutionPin {
    fn drop(&mut self) {
        let mut ledger = self.store.ledger.lock().unwrap_or_else(|p| p.into_inner());
        let Some(slot) = ledger.agents.get_mut(self.identity.agent()) else {
            return;
        };
        if !slot.admitted.is_some_and(|owner| {
            self.identity.allows(
                self.store.as_ref() as *const AgentContexts as usize,
                self.identity.agent(),
                owner,
            )
        }) {
            return;
        }
        slot.execution_pins = slot.execution_pins.saturating_sub(1);
        if slot.execution_pins == 0 {
            slot.execution_reservation = 0;
            let old = slot.charge;
            slot.charge = slot.data.as_ref().map_or(old, AgentData::charge);
            let next = slot.charge;
            if slot.admission_released {
                slot.admitted = None;
            }
            ledger.charge = ledger.charge - old + next;
        }
    }
}

impl AgentExecutionPermit {
    pub(super) fn allows(&self, store: &AgentContexts, agent: &str, owner: AdmissionOwner) -> bool {
        self.identity
            .allows(store as *const AgentContexts as usize, agent, owner)
    }
}

pub(crate) fn with_agent_context_permit(
    contexts: &AgentContexts,
    document: &Path,
    state: &mut EditorState,
    line: &str,
    profile: McpAccessProfile,
    permit: &AgentExecutionPermit,
    action: impl FnOnce(&mut EditorState) -> Result<Option<String>, McpServeError>,
) -> Result<Option<String>, McpServeError> {
    let Some(call) = op_mcp::parse_tool_call(line.trim()) else {
        return Err(McpServeError::Dispatch(
            "group admission requires a parsed tool call".into(),
        ));
    };
    if document != permit.document
        || call.arguments.get("agentId").map(String::as_str) != Some(permit.identity.agent())
    {
        return refusal(&call, AgentContextError::Busy.to_string());
    }
    super::with_agent_context_impl(
        contexts,
        document,
        state,
        line,
        profile,
        Some(permit),
        action,
    )
}

impl std::ops::Deref for AgentExecutionPermit {
    type Target = ExecutionPin;
    fn deref(&self) -> &ExecutionPin {
        &self.inner
    }
}
#[path = "agent_context_delivery.rs"]
mod delivery;
pub(super) use delivery::permit as delivery_permit;
pub(crate) use delivery::Scope as AgentDeliveryScope;
