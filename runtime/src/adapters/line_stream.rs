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
        if !drain_lines(&mut buffer, &mut on_line) {
            return;
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

/// Extract each `\n`-terminated line from `buffer`, calling `on_line` with a
/// slice that includes the trailing `\n`, then compact the consumed prefix
/// once. Any unterminated bytes are left in `buffer` for the next chunk (or
/// the EOF flush). Returns `false` if `on_line` requested an early stop.
fn drain_lines<F>(buffer: &mut Vec<u8>, on_line: &mut F) -> bool
where
    F: FnMut(&[u8]) -> bool,
{
    let mut start = 0usize;
    while let Some(offset) = buffer[start..].iter().position(|&byte| byte == b'\n') {
        let end = start + offset;
        if !on_line(&buffer[start..=end]) {
            return false;
        }
        start = end + 1;
    }
    if start > 0 {
        // Compact once per chunk rather than once per line.
        buffer.drain(..start);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::drain_lines;

    #[test]
    fn two_lines_in_one_input() {
        let mut buffer = b"one\ntwo\n".to_vec();
        let mut lines: Vec<Vec<u8>> = Vec::new();
        let mut on_line = |line: &[u8]| {
            lines.push(line.to_vec());
            true
        };
        assert!(drain_lines(&mut buffer, &mut on_line));
        assert_eq!(lines, vec![b"one\n".to_vec(), b"two\n".to_vec()]);
        assert!(buffer.is_empty());
    }

    #[test]
    fn a_line_spanning_two_inputs() {
        let mut buffer = b"par".to_vec();
        let lines = std::cell::RefCell::new(Vec::<Vec<u8>>::new());
        let mut on_line = |line: &[u8]| {
            lines.borrow_mut().push(line.to_vec());
            true
        };
        assert!(drain_lines(&mut buffer, &mut on_line));
        assert!(lines.borrow().is_empty());
        assert_eq!(buffer, b"par".to_vec());

        buffer.extend_from_slice(b"tial\nnext");
        assert!(drain_lines(&mut buffer, &mut on_line));
        assert_eq!(*lines.borrow(), vec![b"partial\n".to_vec()]);
        assert_eq!(buffer, b"next".to_vec());
    }

    #[test]
    fn an_unterminated_trailing_input_at_eof() {
        let mut buffer = b"line\ntrailing".to_vec();
        let lines = std::cell::RefCell::new(Vec::<Vec<u8>>::new());
        let mut on_line = |line: &[u8]| {
            lines.borrow_mut().push(line.to_vec());
            true
        };
        assert!(drain_lines(&mut buffer, &mut on_line));
        assert_eq!(*lines.borrow(), vec![b"line\n".to_vec()]);
        // What remains in `buffer` is exactly what the EOF flush hands to
        // `on_line` next, unterminated.
        assert_eq!(buffer, b"trailing".to_vec());
        on_line(&buffer);
        assert_eq!(
            *lines.borrow(),
            vec![b"line\n".to_vec(), b"trailing".to_vec()]
        );
    }

    #[test]
    fn stopping_early_leaves_the_rest_unconsumed() {
        let mut buffer = b"a\nb\nc\n".to_vec();
        let mut seen: Vec<Vec<u8>> = Vec::new();
        let mut on_line = |line: &[u8]| {
            seen.push(line.to_vec());
            seen.len() < 2
        };
        assert!(!drain_lines(&mut buffer, &mut on_line));
        assert_eq!(seen, vec![b"a\n".to_vec(), b"b\n".to_vec()]);
    }
}
