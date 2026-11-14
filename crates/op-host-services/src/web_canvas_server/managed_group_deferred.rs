//! Assemble an owned primary cohort and its unexecuted isolated fallback.
use super::*;
pub(super) fn install(
    factory: super::super::async_completion::Factory,
    ticket: queue::Ticket<Request, Reply>,
    deferred_line: String,
    deferred_pin: AgentExecutionPermit,
    path: &Path,
    document: &Path,
    profile: McpAccessProfile,
    fence: crate::document_recovery::DocumentRecovery,
    recovery_epoch: u64,
    generation: u64,
) {
    let source = std::sync::Arc::clone(&factory.source);
    let run_source = std::sync::Arc::clone(&source);
    let panic_fence = fence.clone();
    let path = path.to_path_buf();
    let document = document.to_path_buf();
    let line = deferred_line;
    let pin = deferred_pin;
    let pending = ticket.execute_async(
        move |requests| {
            run_source
                .execute(|ctx| Ok(run_group(ctx, &ctx.authority.managed_groups, requests)))
                .unwrap_or_else(|reason| {
                    requests
                        .iter()
                        .map(|request| Reply::Complete(Ok(error(&request.line, &reason))))
                        .collect()
                })
        },
        move |_| {
            panic_fence.trip_at(recovery_epoch);
            Reply::Complete(Err(WebCanvasError::Config(
                GroupError::LeaderPanicked.to_string(),
            )))
        },
    );
    factory.install(async move {
        let (reply, retained) = pending.await.into_parts();
        let body = match reply {
            Reply::Complete(reply) => reply?,
            Reply::Isolated => {
                let fallback_pin = pin.clone();
                let fallback_fence = fence.clone();
                queue::execute_isolated(move || {
                    source.execute(|ctx| {
                        let _actor =
                            mcp_serve::agent_context::AgentDeliveryScope::enter(fallback_pin);
                        super::super::managed_commit::dispatch_pending(
                            ctx,
                            &line,
                            profile,
                            Some(&path),
                            &document,
                            (recovery_epoch, generation),
                        )
                    })
                })
                .await
                .map_err(|_| {
                    fallback_fence.trip_at(recovery_epoch);
                    WebCanvasError::Config(GroupError::LeaderPanicked.to_string())
                })??
            }
        };
        Ok(super::super::async_completion::Response {
            body,
            _retained: Box::new((retained, pin)),
        })
    });
}
