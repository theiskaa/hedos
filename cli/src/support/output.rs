//! Output helpers: human-readable lines on stdout, notices on stderr, streamed
//! tokens without a trailing newline, and machine-readable JSON under `--json`.
//!
//! A pipe whose reader left (`hedos run ... | head -c 1`) closes stdout for
//! the rest of the run: later writes are dropped, and [`stdout_closed`]
//! resolves so `main` stops the command the way a termination stops it, the
//! model servers it started with it. A write to a terminal that hung up or
//! was revoked is dropped too, since saying so would only fail the same way.
//! Any other failed write to stdout, such as one to a full disk, is
//! remembered, so the command does not report success over output it did not
//! finish (see [`write_failure`]).

use std::io::{self, Write};
use std::sync::OnceLock;

use tokio::sync::watch;

/// The first failed write to stdout that [`terminal_gone`] does not explain.
static WRITE_FAILURE: OnceLock<String> = OnceLock::new();

/// Whether a write to stdout has found the pipe's reader gone.
static CLOSED: OnceLock<watch::Sender<bool>> = OnceLock::new();

/// The output mode, threaded from the global `--json` flag.
#[derive(Clone, Copy)]
pub struct Out {
    json: bool,
}

impl Out {
    /// An output sink in human (`json = false`) or JSON mode.
    pub fn new(json: bool) -> Self {
        Self { json }
    }

    /// Whether machine-readable JSON was requested.
    pub fn is_json(&self) -> bool {
        self.json
    }

    /// A line of human output on stdout (suppressed in JSON mode).
    pub fn line(&self, text: &str) {
        if !self.json {
            stdout(text, true);
        }
    }

    /// Raw output on stdout with no newline, flushed, for streamed tokens.
    pub fn raw(&self, text: &str) {
        if !self.json {
            stdout(text, false);
        }
    }

    /// A notice on stderr (status, prompts), always shown.
    pub fn err(&self, text: &str) {
        let _ = writeln!(io::stderr(), "{text}");
    }

    /// A JSON document on stdout (pretty-printed). A no-op outside JSON mode.
    pub fn json(&self, value: &serde_json::Value) {
        if self.json {
            match serde_json::to_string_pretty(value) {
                Ok(text) => stdout(&text, true),
                Err(error) => self.err(&error.to_string()),
            }
        }
    }
}

/// Whether `error` says the terminal or pipe being written to has gone: a
/// pipe whose reader left, or a terminal that hung up or was revoked.
pub fn terminal_gone(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::BrokenPipe
        || matches!(error.raw_os_error(), Some(libc::EIO | libc::ENXIO))
}

/// Why a write to stdout failed, for the first failure that was not to a
/// terminal or pipe that had gone, if any.
pub fn write_failure() -> Option<&'static str> {
    WRITE_FAILURE.get().map(String::as_str)
}

/// Resolve once a write to stdout has found the pipe's reader gone.
pub async fn stdout_closed() {
    let mut closed = closing().subscribe();
    if closed.wait_for(|closed| *closed).await.is_err() {
        std::future::pending::<()>().await;
    }
}

fn closing() -> &'static watch::Sender<bool> {
    CLOSED.get_or_init(|| watch::channel(false).0)
}

fn stdout(text: &str, newline: bool) {
    let closed = closing();
    if *closed.borrow() {
        return;
    }
    if let Err(error) = write_text(&mut io::stdout().lock(), text, newline) {
        match failure(&error) {
            Failure::Closed => {
                closed.send_replace(true);
            }
            Failure::Gone => {}
            Failure::Other => {
                let _ = WRITE_FAILURE.set(error.to_string());
            }
        }
    }
}

/// What a failed write to stdout means for the command.
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    /// The pipe's reader left: nothing will read another line.
    Closed,
    /// The terminal hung up or was revoked, which its hangup handles.
    Gone,
    /// The output was not written, and the command must not claim success.
    Other,
}

fn failure(error: &io::Error) -> Failure {
    if error.kind() == io::ErrorKind::BrokenPipe {
        Failure::Closed
    } else if terminal_gone(error) {
        Failure::Gone
    } else {
        Failure::Other
    }
}

/// Write `text`, and a newline when asked, then flush, so a failure surfaces
/// on the write that caused it rather than on a later one.
fn write_text(sink: &mut impl Write, text: &str, newline: bool) -> io::Result<()> {
    sink.write_all(text.as_bytes())?;
    if newline {
        sink.write_all(b"\n")?;
    }
    sink.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_gone_terminal_or_pipe_is_forgiven() {
        assert!(terminal_gone(&io::Error::from(io::ErrorKind::BrokenPipe)));
        assert!(terminal_gone(&io::Error::from_raw_os_error(libc::EIO)));
        assert!(terminal_gone(&io::Error::from_raw_os_error(libc::ENXIO)));
        assert!(!terminal_gone(&io::Error::from_raw_os_error(libc::ENOSPC)));
        assert!(!terminal_gone(&io::Error::from_raw_os_error(libc::EFBIG)));
    }

    #[test]
    fn only_a_closed_pipe_stops_the_command_and_only_other_failures_fail_it() {
        assert_eq!(
            failure(&io::Error::from_raw_os_error(libc::EPIPE)),
            Failure::Closed
        );
        assert_eq!(
            failure(&io::Error::from_raw_os_error(libc::EIO)),
            Failure::Gone
        );
        assert_eq!(
            failure(&io::Error::from_raw_os_error(libc::ENXIO)),
            Failure::Gone
        );
        assert_eq!(
            failure(&io::Error::from_raw_os_error(libc::EFBIG)),
            Failure::Other
        );
        assert_eq!(
            failure(&io::Error::from_raw_os_error(libc::ENOSPC)),
            Failure::Other
        );
    }

    struct Gone;

    impl Write for Gone {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_write_to_a_closed_pipe_reports_the_closed_pipe() {
        let error = write_text(&mut Gone, "row", true).unwrap_err();
        assert_eq!(failure(&error), Failure::Closed);
    }

    #[test]
    fn a_line_ends_with_a_newline_and_raw_text_does_not() {
        let mut sink = Vec::new();
        write_text(&mut sink, "a", true).unwrap();
        write_text(&mut sink, "b", false).unwrap();
        assert_eq!(sink, b"a\nb");
    }
}
