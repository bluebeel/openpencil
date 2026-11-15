//! Nonblocking SSE writer on the existing shared runtime. One latest tick and
//! one in-progress frame bound user-space buffering independently of publishers.

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::watch;

use super::Ready;
use crate::web_canvas_server::{sse_wire, Result, WebCanvasError, IO_TIMEOUT, SSE_HEARTBEAT};

pub(super) async fn serve(
    mut socket: TcpStream,
    ready: Ready,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    // Destructure deliberately: the otherwise-unused lease is held until this
    // future exits, not dropped when its last borrowed field is read.
    let Ready {
        slot,
        current,
        cors_origin,
        tenant: _tenant,
        online: _online,
    } = ready;
    if !write_frame(
        &mut socket,
        &mut shutdown,
        sse_wire::headers(cors_origin.as_deref()).as_bytes(),
        "headers",
    )
    .await?
        || !write_frame(
            &mut socket,
            &mut shutdown,
            sse_wire::event(current).as_bytes(),
            "write",
        )
        .await?
    {
        return Ok(());
    }
    // Headers have been sent; do not retain the origin String for the lifetime
    // of an idle stream. Admission already bounded the HTTP header size.
    drop(cors_origin);
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        let heartbeat = tokio::select! {
            biased;
            _ = shutdown.changed() => return Ok(()),
            _ = slot.notified_async() => false,
            _ = tokio::time::sleep(SSE_HEARTBEAT) => true,
        };
        // Consume only AFTER selection. Taking a tick inside a cancelable
        // future can discard it when shutdown/heartbeat wins the same poll.
        let event = slot.take_pending().map(sse_wire::event);
        let bytes = match event.as_deref() {
            Some(event) => event.as_bytes(),
            None if heartbeat => b": ping\n\n",
            None => continue, // At most one stale Notify permit; no polling.
        };
        if !write_frame(&mut socket, &mut shutdown, bytes, "write").await? {
            return Ok(());
        }
    }
}

/// A paused reader cannot keep a task/lease forever in a blocked write.
/// Shutdown interrupts writes immediately rather than waiting for the timeout.
/// We intentionally do not read EOF: a client may half-close its request side
/// while continuing to read SSE, which the synchronous endpoint also supports.
async fn write_frame(
    socket: &mut TcpStream,
    shutdown: &mut watch::Receiver<bool>,
    bytes: &[u8],
    phase: &'static str,
) -> Result<bool> {
    if *shutdown.borrow() {
        return Ok(false);
    }
    let write = async {
        socket.write_all(bytes).await?;
        socket.flush().await
    };
    tokio::select! {
        biased;
        _ = shutdown.changed() => Ok(false),
        result = tokio::time::timeout(IO_TIMEOUT, write) => {
            match result {
                Ok(Ok(())) => Ok(true),
                Ok(Err(error)) => Err(WebCanvasError::Transport(format!("sse {phase}: {error}"))),
                Err(_) => Err(WebCanvasError::Transport(format!("sse {phase}: write timed out"))),
            }
        }
    }
}
