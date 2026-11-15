//! One idle-blocking reaper for already-signaled local ACP child handles.

use std::sync::{mpsc, OnceLock};
use std::time::Duration;

use tokio::process::Child;

const REAP_INTERVAL: Duration = Duration::from_millis(10);
const QUEUED_CHILDREN: usize = 4096;

pub(super) fn enqueue(child: Child) {
    static REAPER: OnceLock<Option<mpsc::SyncSender<Child>>> = OnceLock::new();
    let sender = REAPER.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel(QUEUED_CHILDREN);
        std::thread::Builder::new()
            .name("op-acp-child-reaper".into())
            .spawn(move || run(receiver))
            .ok()
            .map(|_| sender)
    });
    let child = match sender {
        Some(sender) => match sender.try_send(child) {
            Ok(()) => return,
            Err(mpsc::TrySendError::Full(child) | mpsc::TrySendError::Disconnected(child)) => child,
        },
        None => child,
    };
    // Preserve the old exact-handle fallback if a thread cannot start, the
    // bounded ingress fills, or the shared reaper has unexpectedly failed.
    let _ = std::thread::Builder::new()
        .name("op-acp-child-reaper-fallback".into())
        .spawn(move || run_one(child));
}

fn run(receiver: mpsc::Receiver<Child>) {
    let mut children: Vec<Child> = Vec::new();
    loop {
        let next = if children.is_empty() {
            match receiver.recv() {
                Ok(child) => Some(child),
                Err(_) => return,
            }
        } else {
            match receiver.recv_timeout(REAP_INTERVAL) {
                Ok(child) => Some(child),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    for child in children {
                        run_one(child);
                    }
                    return;
                }
            }
        };
        children.extend(next);
        // Only already-signaled handles enter this queue. Work is bounded by
        // those owned children; the worker sleeps indefinitely when idle.
        children.extend(receiver.try_iter().take(QUEUED_CHILDREN));
        children.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
    }
}

fn run_one(mut child: Child) {
    while matches!(child.try_wait(), Ok(None)) {
        std::thread::sleep(REAP_INTERVAL);
    }
}
