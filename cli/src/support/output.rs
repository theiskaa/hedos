//! Output helpers: human-readable lines on stdout, notices on stderr, streamed
//! tokens without a trailing newline, and machine-readable JSON under `--json`.
//! A write to a terminal or pipe that has gone is dropped: it takes no more
//! output, and saying so would only fail the same way. Any other failed write
//! to stdout, such as one to a full disk, is remembered, so the command does
//! not report success over output it did not finish (see [`write_failure`]).

use std::io::{self, Write};
use std::sync::OnceLock;

/// The first failed write to stdout that [`terminal_gone`] does not explain.
static WRITE_FAILURE: OnceLock<String> = OnceLock::new();

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
            remember(writeln!(io::stdout(), "{text}"));
        }
    }

    /// Raw output on stdout with no newline, flushed — for streamed tokens.
    pub fn raw(&self, text: &str) {
        if !self.json {
            let mut stdout = io::stdout();
            remember(write!(stdout, "{text}").and_then(|()| stdout.flush()));
        }
    }

    /// A notice on stderr (status, prompts) — always shown.
    pub fn err(&self, text: &str) {
        let _ = writeln!(io::stderr(), "{text}");
    }

    /// A JSON document on stdout (pretty-printed). A no-op outside JSON mode.
    pub fn json(&self, value: &serde_json::Value) {
        if self.json {
            match serde_json::to_string_pretty(value) {
                Ok(text) => remember(writeln!(io::stdout(), "{text}")),
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

fn remember(written: io::Result<()>) {
    if let Err(error) = written
        && !terminal_gone(&error)
    {
        let _ = WRITE_FAILURE.set(error.to_string());
    }
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
}
