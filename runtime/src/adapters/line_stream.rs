//! A shared newline-delimited reader for the HTTP streaming adapters. It reads a
//! `reqwest::Response` chunk by chunk, splits it into `\n`-terminated lines, and
//! hands each to a callback — enforcing response/line size bounds and stopping
//! promptly when the consumer drops (cancel-safe via `tx.closed()`).

use kernel::capabilities::CapabilityChunk;
use tokio::sync::mpsc;

use super::RuntimeError;

/// The largest total response body read before it is rejected as oversized.
pub(crate) const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
/// The largest single unterminated line buffered before it is rejected.
pub(crate) const MAX_LINE_BYTES: usize = 2 * 1024 * 1024;

type Sender = mpsc::UnboundedSender<Result<CapabilityChunk, RuntimeError>>;

/// Drain `response` line by line, calling `on_line` with each `\n`-terminated
/// line and once with the trailing partial at EOF. `on_line` returns `false` to
/// stop early. Oversized bodies/lines and transport errors are reported on `tx`
/// with `error_prefix` naming the source; a dropped consumer ends the read at
/// once.
pub(crate) async fn read_lines<F>(
    mut response: reqwest::Response,
    tx: &Sender,
    error_prefix: &str,
    mut on_line: F,
) where
    F: FnMut(&[u8]) -> bool,
{
    let mut buffer: Vec<u8> = Vec::new();
    let mut total = 0usize;
    loop {
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk,
            _ = tx.closed() => return,
        };
        match chunk {
            Ok(Some(bytes)) => {
                total += bytes.len();
                if total > MAX_RESPONSE_BYTES {
                    let _ = tx.send(Err(RuntimeError::Failed(
                        "the server sent an oversized response".to_owned(),
                    )));
                    return;
                }
                buffer.extend_from_slice(&bytes);
            }
            Ok(None) => break,
            Err(err) => {
                let _ = tx.send(Err(RuntimeError::Failed(format!("{error_prefix}: {err}"))));
                return;
            }
        }
        while let Some(newline) = buffer.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = buffer.drain(..=newline).collect();
            if !on_line(&line) {
                return;
            }
        }
        // A single line that never terminates must not buffer without bound.
        if buffer.len() > MAX_LINE_BYTES {
            let _ = tx.send(Err(RuntimeError::Failed(
                "the server sent an oversized line".to_owned(),
            )));
            return;
        }
    }
    on_line(&buffer);
}
