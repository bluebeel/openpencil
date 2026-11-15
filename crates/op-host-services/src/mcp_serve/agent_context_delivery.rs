//! Exact transferred admission for an asynchronous unexecuted fallback. The
//! shared pin retains actor UI reservation and admission through socket reply.
use super::*;
use std::{cell::RefCell, rc::Rc};
thread_local! { static CURRENT: RefCell<Vec<Rc<AgentExecutionPermit>>> = const { RefCell::new(Vec::new()) }; }
pub(crate) struct Scope {
    frame: Rc<AgentExecutionPermit>,
}
impl Scope {
    pub(crate) fn enter(permit: AgentExecutionPermit) -> Self {
        let frame = Rc::new(permit);
        CURRENT.with(|current| current.borrow_mut().push(Rc::clone(&frame)));
        Self { frame }
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|current| {
            let mut current = current.borrow_mut();
            if current
                .last()
                .is_some_and(|frame| Rc::ptr_eq(frame, &self.frame))
            {
                current.pop();
            } else if let Some(index) = current
                .iter()
                .rposition(|frame| Rc::ptr_eq(frame, &self.frame))
            {
                current.remove(index);
            }
        });
    }
}
pub(in crate::mcp_serve::agent_context) fn permit(
    contexts: &AgentContexts,
    document: &Path,
    line: &str,
) -> Option<(AgentExecutionPermit, op_mcp::ToolCall)> {
    CURRENT.with(|current| {
        let current = current.borrow();
        // Ordinary calls have no transferred admission. Do not parse their
        // entire structured argument payload just to discover an empty stack.
        if !current.iter().any(|permit| {
            std::ptr::eq(permit.store.as_ref(), contexts) && permit.document == document
        }) {
            return None;
        }
        let call = op_mcp::parse_tool_call(line)?;
        let agent = call.arguments.get("agentId")?;
        let permit = current
            .iter()
            .rev()
            .find(|permit| {
                std::ptr::eq(permit.store.as_ref(), contexts)
                    && permit.document == document
                    && permit.identity.agent() == agent.as_str()
            })?
            .as_ref()
            .clone();
        Some((permit, call))
    })
}
