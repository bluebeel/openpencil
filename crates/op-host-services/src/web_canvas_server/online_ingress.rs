//! Bounded framing/authentication workers hand verified requests to fair owners.
use super::{request, scheduler, *};
use std::{
    net::TcpStream,
    sync::{atomic::Ordering, Arc, Weak},
};

pub(super) struct Context {
    pub registry: Arc<TenantRegistry>,
    pub verifier: Arc<dyn IdentityVerifier>,
    pub write_barrier: Arc<super::super::tenant::WriteBarrier>,
    pub shutdown: Arc<AtomicBool>,
    pub streams: Arc<async_sse::Streams>,
    pub queue: Weak<scheduler::Queue>,
    pub bound: u16,
}
impl Context {
    pub(super) fn receive(self: &Arc<Self>, mut socket: TcpStream, connection: ConnGuard) {
        let _ = socket.set_read_timeout(Some(IO_TIMEOUT));
        let _ = socket.set_write_timeout(Some(IO_TIMEOUT));
        // Preserve the pre-dispatch shutdown refusal for sockets queued before
        // the final writer barrier. No parser/document execution follows it.
        if self.shutdown.load(Ordering::Acquire) {
            refuse(&mut socket, "server shutting down", None);
            return;
        }
        let mut ready = match request::prepare(&mut socket, &self.registry, self.verifier.as_ref())
        {
            Ok(request::Prepared::Ready(ready)) => ready,
            Ok(request::Prepared::Complete(done)) => {
                self.complete(done);
                return;
            }
            Err(error) => {
                eprintln!("openpencil --serve-web --online: {error}");
                return;
            }
        };
        let bytes = ready.queue_bytes();
        let owner = std::mem::take(&mut ready.owner);
        let Some(queue) = self.queue.upgrade() else {
            refuse(&mut socket, "server shutting down", None);
            return;
        };
        let context = Arc::clone(self);
        queue.submit(
            &owner,
            bytes,
            Box::new(move |admission| {
                let cors_origin = online_policy::online_cors_origin(
                    context.registry.allow_origins(),
                    ready.req.origin.as_deref(),
                );
                let permit = match admission {
                    Ok(permit) => permit,
                    Err(reason) => {
                        refuse(&mut socket, reason.message(), cors_origin.as_deref());
                        return;
                    }
                };
                if context.shutdown.load(Ordering::Acquire) {
                    refuse(&mut socket, "server shutting down", cors_origin.as_deref());
                    return;
                }
                let mut permit = Some(permit);
                let outcome =
                    async_sse::serve_socket(socket, connection, &context.streams, |stream| {
                        let result = request::dispatch(
                            stream,
                            ready,
                            &context.registry,
                            &context.write_barrier,
                        );
                        if result.is_ok() {
                            // Only a captured, authorized SSE route keeps this permit.
                            // Ordinary responses release it before this worker returns.
                            async_sse::retain_online_admission(permit.take().unwrap());
                        }
                        result
                    });
                match outcome {
                    Ok(done) => context.complete(done),
                    Err(error) => eprintln!("openpencil --serve-web --online: {error}"),
                }
            }),
        );
    }
    fn complete(&self, done: bool) {
        if done {
            self.shutdown.store(true, Ordering::Release);
            let _ = TcpStream::connect(("127.0.0.1", self.bound));
        }
    }
}
fn refuse(socket: &mut TcpStream, message: &str, origin: Option<&str>) {
    let _ = crate::mcp_serve::write_mcp_http_response_with_origin(
        socket,
        "503 Service Unavailable",
        &serde_json::json!({"ok":false,"error":message}).to_string(),
        origin,
    );
}
