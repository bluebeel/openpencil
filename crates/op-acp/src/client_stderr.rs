//! Drain arbitrary child output without an unbounded line allocation.

use std::sync::{Arc, Mutex};

use op_util::cli_output::BoundedTail;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

use super::STDERR_TAIL_CAP;

pub(super) async fn drain(mut reader: impl AsyncBufRead + Unpin, tail: Arc<Mutex<BoundedTail>>) {
    let mut prefix = Vec::new();
    let mut truncated = false;
    loop {
        let available = match reader.fill_buf().await {
            Ok(available) => available,
            Err(_) => return,
        };
        if available.is_empty() {
            if !prefix.is_empty() {
                retain_line(&prefix, truncated, &tail);
            }
            return;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let end = newline.unwrap_or(available.len());
        let retained = end.min(STDERR_TAIL_CAP - prefix.len());
        prefix.extend_from_slice(&available[..retained]);
        truncated |= retained < end;
        reader.consume(end + usize::from(newline.is_some()));
        if newline.is_some() {
            if !truncated && prefix.last() == Some(&b'\r') {
                prefix.pop();
            }
            retain_line(&prefix, truncated, &tail);
            prefix.clear();
            truncated = false;
        }
    }
}

fn retain_line(prefix: &[u8], truncated: bool, tail: &Mutex<BoundedTail>) {
    let line = match std::str::from_utf8(prefix) {
        Ok(line) => line,
        // A valid character may straddle the bounded prefix. Discard only
        // that partial character; genuinely malformed lines are not surfaced.
        Err(error) if truncated && error.error_len().is_none() => {
            std::str::from_utf8(&prefix[..error.valid_up_to()]).expect("validated UTF-8 prefix")
        }
        Err(_) => return,
    };
    if let Ok(mut tail) = tail.lock() {
        tail.push_line(line);
    }
}
