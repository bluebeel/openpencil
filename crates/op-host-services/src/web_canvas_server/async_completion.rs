//! Production-only deferred durable replies. Authentication/validation still
//! runs on the original worker; accepted queue futures retain exact capabilities.
use super::{
    connection::ConnCtx, ConnAuthority, ConnGuard, Result, ServeMode, SseHub, WebCanvasError,
    WebCanvasState,
};
use std::{
    cell::RefCell,
    future::Future,
    net::TcpStream,
    pin::Pin,
    rc::Rc,
    sync::{Arc, Condvar, Mutex},
};
use tokio::io::AsyncWriteExt;

pub(crate) struct Source {
    state: Arc<Mutex<WebCanvasState>>,
    authority: Arc<ConnAuthority>,
    hub: Arc<SseHub>,
    mode: ServeMode,
}
impl Source {
    pub(crate) fn execute<R>(&self, run: impl FnOnce(&ConnCtx<'_>) -> Result<R>) -> Result<R> {
        let _binding = if self.mode.allows_local_file_routes() {
            Some(
                super::primary_scope::Scope::read(
                    Arc::clone(&self.authority.primary_files),
                    self.authority.recovery.clone(),
                )
                .map_err(|error| WebCanvasError::Config(error.to_string()))?,
            )
        } else {
            None
        };
        run(&ConnCtx {
            state: &self.state,
            authority: &self.authority,
            hub: &self.hub,
            mode: self.mode,
            mcp_profile: crate::mcp_serve::tool_profile::McpAccessProfile::UNRESTRICTED,
            rest_identity: None,
            write_barrier: None,
        })
    }
}
thread_local! {
    static SOURCE: RefCell<Option<Arc<Source>>> = const { RefCell::new(None) };
    static CURRENT: RefCell<Option<Rc<RefCell<Frame>>>> = const { RefCell::new(None) };
}
struct SourceScope(Option<Arc<Source>>);
impl Drop for SourceScope {
    fn drop(&mut self) {
        SOURCE.with(|source| {
            source.replace(self.0.take());
        });
    }
}
pub(super) fn with_primary<R>(
    state: Arc<Mutex<WebCanvasState>>,
    authority: Arc<ConnAuthority>,
    hub: Arc<SseHub>,
    mode: ServeMode,
    run: impl FnOnce() -> R,
) -> R {
    let old = SOURCE.with(|source| {
        source.replace(Some(Arc::new(Source {
            state,
            authority,
            hub,
            mode,
        })))
    });
    let _scope = SourceScope(old);
    run()
}

pub(crate) struct Response {
    pub(crate) body: String,
    pub(crate) _retained: Box<dyn Send>,
}
type Pending = Pin<Box<dyn Future<Output = Result<Response>> + Send + 'static>>;
struct Frame {
    source: Option<Arc<Source>>,
    origin: Option<String>,
    ready: Option<Ready>,
}
pub(super) struct Ready {
    pending: Pending,
    origin: Option<String>,
}
pub(super) struct Capture {
    frame: Rc<RefCell<Frame>>,
    previous: Option<Rc<RefCell<Frame>>>,
}
impl Capture {
    pub(super) fn enter() -> Self {
        let frame = Rc::new(RefCell::new(Frame {
            source: SOURCE.with(|source| source.borrow().clone()),
            origin: None,
            ready: None,
        }));
        let previous = CURRENT.with(|current| current.replace(Some(Rc::clone(&frame))));
        Self { frame, previous }
    }
    pub(super) fn finish(self) -> Option<Ready> {
        self.frame.borrow_mut().ready.take()
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        CURRENT.with(|current| {
            current.replace(self.previous.take());
        });
    }
}
pub(super) fn origin(origin: Option<&str>) {
    CURRENT.with(|current| {
        if let Some(frame) = current.borrow().as_ref() {
            frame.borrow_mut().origin = origin.map(str::to_owned);
        }
    });
}
/// Obtain before submission. A factory exists only on an owned production TCP
/// dispatch; generic synchronous APIs keep their existing behavior.
pub(crate) struct Factory {
    frame: Rc<RefCell<Frame>>,
    pub(crate) source: Arc<Source>,
}
pub(crate) fn prepare() -> Option<Factory> {
    CURRENT.with(|current| {
        let frame = current.borrow().as_ref()?.clone();
        let source = {
            let state = frame.borrow();
            if state.ready.is_some() {
                return None;
            }
            state.source.clone()?
        };
        Some(Factory { frame, source })
    })
}
impl Factory {
    pub(crate) fn install(self, pending: impl Future<Output = Result<Response>> + Send + 'static) {
        let mut frame = self.frame.borrow_mut();
        assert!(frame.ready.is_none(), "one deferred response per request");
        frame.ready = Some(Ready {
            pending: Box::pin(pending),
            origin: frame.origin.clone(),
        });
    }
}

#[derive(Default)]
struct FlightState {
    active: usize,
    closed: bool,
}
#[derive(Default)]
struct Shared {
    state: Mutex<FlightState>,
    drained: Condvar,
}
#[derive(Default)]
pub(super) struct Flights {
    shared: Arc<Shared>,
}
struct Flight(Arc<Shared>);
impl Drop for Flight {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        state.active -= 1;
        if state.active == 0 {
            self.0.drained.notify_all();
        }
    }
}
impl Flights {
    pub(super) fn close(&self) {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        state.closed = true;
        // Accepted writes finish commit/rollback and bounded socket delivery.
        // Closing the listener does not turn claimed work into a retry signal.
        while state.active != 0 {
            state = self
                .shared
                .drained
                .wait(state)
                .unwrap_or_else(|p| p.into_inner());
        }
    }
    pub(super) fn handoff(
        &self,
        socket: TcpStream,
        connection: ConnGuard,
        ready: Ready,
    ) -> Result<()> {
        let flight = {
            let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.closed {
                return Ok(());
            }
            state.active += 1;
            Flight(Arc::clone(&self.shared))
        };
        socket
            .set_read_timeout(None)
            .and_then(|()| socket.set_write_timeout(None))
            .and_then(|()| socket.set_nonblocking(true))
            .map_err(|error| WebCanvasError::Transport(format!("durable reply socket: {error}")))?;
        let runtime = crate::chat_runtime::shared_runtime();
        let socket = {
            let _entered = runtime.enter();
            tokio::net::TcpStream::from_std(socket).map_err(|error| {
                WebCanvasError::Transport(format!("durable reply socket: {error}"))
            })?
        };
        runtime.spawn(async move {
            let _flight = flight;
            let _connection = connection;
            if let Err(error) = deliver(socket, ready).await {
                eprintln!("openpencil --serve-web: {error}");
            }
        });
        Ok(())
    }
}
async fn deliver(mut socket: tokio::net::TcpStream, ready: Ready) -> Result<()> {
    // Do not cancel accepted work on network EOF: its outcome must finish once.
    // An uncertain executor returns Err and closes without an invented ack.
    let response = ready.pending.await?;
    let cors = ready
        .origin
        .map(|origin| format!("Access-Control-Allow-Origin: {origin}\r\n"))
        .unwrap_or_default();
    let status = if response.body.is_empty() {
        "202 Accepted"
    } else {
        "200 OK"
    };
    let header=format!("HTTP/1.1 {status}\r\n{cors}Access-Control-Allow-Methods: GET, POST, DELETE, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, mcp-session-id, X-OpenPencil-Token, Authorization\r\nAccess-Control-Expose-Headers: mcp-session-id\r\nmcp-session-id: openpencil\r\nCache-Control: no-store\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",response.body.len());
    let write = async {
        socket.write_all(header.as_bytes()).await?;
        socket.write_all(response.body.as_bytes()).await?;
        socket.flush().await
    };
    let result = tokio::time::timeout(super::IO_TIMEOUT, write).await;
    // `response._retained` keeps queue bytes + exact agent admission until here.
    drop(response);
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(WebCanvasError::Transport(format!("durable reply: {error}"))),
        Err(_) => Err(WebCanvasError::Transport(
            "durable reply: write timed out".into(),
        )),
    }
}

pub(crate) fn captured_empty_response(body: &str) -> bool {
    body.is_empty()
        && CURRENT.with(|current| {
            current
                .borrow()
                .as_ref()
                .is_some_and(|frame| frame.borrow().ready.is_some())
        })
}
