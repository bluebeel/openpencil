//! Authorized TCP SSE requests leave the blocking request-worker pool here.
//!
//! Only production socket dispatch installs a capture. Generic `Read + Write`
//! callers retain their synchronous behavior. The capture never crosses a
//! thread boundary: only the subscribed slot, initial tick, and (online) exact
//! tenant lease move into the shared runtime's socket task.

use std::cell::RefCell;
use std::net::TcpStream;
use std::rc::Rc;
use std::sync::Arc;

use tokio::sync::watch;

use super::tenant::TenantLease;
use super::{ConnGuard, Result, SseSlot, SseTick, WebCanvasError};

#[path = "async_sse_stream.rs"]
mod stream;

type Captured = Rc<RefCell<Option<Ready>>>;

thread_local! {
    // Request-local transport handoff, not an authorization or tenant lookup.
    static CURRENT: RefCell<Option<Captured>> = const { RefCell::new(None) };
}

struct Ready {
    slot: Arc<SseSlot>,
    current: SseTick,
    cors_origin: Option<String>,
    tenant: Option<TenantLease>,
    online: Option<super::online_run_loop::scheduler::ConnectionPermit>,
}

struct Capture {
    captured: Captured,
    previous: Option<Captured>,
}

impl Capture {
    fn enter() -> Self {
        let captured = Rc::new(RefCell::new(None));
        let previous = CURRENT.with(|current| current.replace(Some(Rc::clone(&captured))));
        Self { captured, previous }
    }

    fn finish(self) -> Option<Ready> {
        let ready = self.captured.borrow_mut().take();
        ready
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // Also runs if parsing or dispatch panics; a reused worker must never
        // capture a later request into the previous request's slot.
        CURRENT.with(|current| {
            current.replace(self.previous.take());
        });
    }
}

/// Called only after the existing Origin, credential, scope, and tenant gates.
/// `false` asks the generic compatibility handler to keep streaming in place.
pub(super) fn capture(slot: &Arc<SseSlot>, current: SseTick, cors_origin: Option<&str>) -> bool {
    CURRENT.with(|scope| {
        let scope = scope.borrow();
        let Some(captured) = scope.as_ref() else {
            return false;
        };
        *captured.borrow_mut() = Some(Ready {
            slot: Arc::clone(slot),
            current,
            cors_origin: cors_origin.map(str::to_owned),
            tenant: None,
            online: None,
        });
        true
    })
}

/// Preserve the same tenant connection/eviction lease as synchronous SSE.
/// For ordinary requests and generic synchronous callers, release it normally.
pub(super) fn retain_tenant(lease: TenantLease) {
    let mut lease = Some(lease);
    CURRENT.with(|scope| {
        let scope = scope.borrow();
        if let Some(captured) = scope.as_ref() {
            if let Some(ready) = captured.borrow_mut().as_mut() {
                ready.tenant = lease.take();
            }
        }
    });
}

/// Retain only the cheap verified-owner admission across an SSE handoff. The
/// executing owner slot stays on the worker and is released when it returns.
pub(super) fn retain_online_admission(permit: super::online_run_loop::scheduler::ConnectionPermit) {
    let mut permit = Some(permit);
    CURRENT.with(|scope| {
        let scope = scope.borrow();
        if let Some(captured) = scope.as_ref() {
            if let Some(ready) = captured.borrow_mut().as_mut() {
                ready.online = permit.take();
            }
        }
    });
}

/// One daemon's cancellation source. Sockets remain charged to the existing
/// connection quota until their async task releases its original `ConnGuard`.
pub(super) struct Streams {
    shutdown: watch::Sender<bool>,
}

impl Streams {
    pub(super) fn new() -> Self {
        let (shutdown, _) = watch::channel(false);
        Self { shutdown }
    }

    pub(super) fn close(&self) {
        // `send_replace` retains closure even when the last task has exited,
        // so a queued request cannot create a new stream after shutdown.
        self.shutdown.send_replace(true);
    }

    fn handoff(&self, socket: TcpStream, connection: ConnGuard, ready: Ready) -> Result<()> {
        let shutdown = self.shutdown.subscribe();
        if *shutdown.borrow() {
            return Ok(());
        }
        socket
            .set_read_timeout(None)
            .and_then(|()| socket.set_write_timeout(None))
            .and_then(|()| socket.set_nonblocking(true))
            .map_err(|error| WebCanvasError::Transport(format!("sse socket: {error}")))?;
        let runtime = crate::chat_runtime::shared_runtime();
        let socket = {
            let _entered = runtime.enter();
            tokio::net::TcpStream::from_std(socket)
                .map_err(|error| WebCanvasError::Transport(format!("sse socket: {error}")))?
        };
        runtime.spawn(async move {
            // Keep both admission and online tenant ownership until the stream
            // ends, including blocked writes and cancellation. No document lock
            // or request body is retained by the socket task.
            let _connection = connection;
            if let Err(error) = stream::serve(socket, ready, shutdown).await {
                eprintln!("openpencil --serve-web: {error}");
            }
        });
        Ok(())
    }
}

/// Dispatch exactly one parsed request, transferring the socket only when its
/// authorized SSE route requested a handoff. All other outcomes retain normal
/// connection/guard destruction, including errors and unwind.
pub(super) fn serve_socket(
    mut socket: TcpStream,
    connection: ConnGuard,
    streams: &Streams,
    dispatch: impl FnOnce(&mut TcpStream) -> Result<bool>,
) -> Result<bool> {
    let capture = Capture::enter();
    let outcome = dispatch(&mut socket);
    let ready = capture.finish();
    match (outcome, ready) {
        (Ok(false), Some(ready)) => {
            streams.handoff(socket, connection, ready)?;
            Ok(false)
        }
        (outcome, _) => outcome,
    }
}
