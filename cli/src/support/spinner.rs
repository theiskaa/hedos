//! An animated loader for the long, quiet setup steps (creating the Python
//! environment, installing packages, loading a model) so they read as working
//! rather than frozen. Wraps `indicatif`, which ticks on its own thread and so
//! keeps animating while the command is blocked awaiting the next status.

use std::io::IsTerminal;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};

use crate::support::output::Out;

/// A single-line spinner showing the current step. On a terminal it animates; on
/// a non-terminal it prints each *distinct* step to stderr so logs still capture
/// progress; under `--json` it is silent, keeping machine runs quiet.
pub struct Spinner {
    bar: Option<ProgressBar>,
    silent: bool,
    last: Option<String>,
}

impl Spinner {
    /// Start a spinner, a plain stderr fallback, or a silent no-op, depending on
    /// whether stderr is a terminal and whether `--json` was asked for.
    pub fn start(out: &Out) -> Self {
        if out.is_json() {
            return Self::inert(true);
        }
        if !std::io::stderr().is_terminal() {
            return Self::inert(false);
        }
        let style = ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner());
        let bar = ProgressBar::new_spinner().with_style(style);
        bar.enable_steady_tick(Duration::from_millis(90));
        Self {
            bar: Some(bar),
            silent: false,
            last: None,
        }
    }

    fn inert(silent: bool) -> Self {
        Self {
            bar: None,
            silent,
            last: None,
        }
    }

    /// Show `message` as the current step. On the stderr fallback, repeats of the
    /// same message are dropped so a stream of identical ticks does not spam a log.
    pub fn set(&mut self, message: &str) {
        if let Some(bar) = &self.bar {
            bar.set_message(message.to_owned());
        } else if !self.silent && self.last.as_deref() != Some(message) {
            eprintln!("{message}");
            self.last = Some(message.to_owned());
        }
    }

    /// Stop the spinner and clear its line. Idempotent, so callers can clear it
    /// before streamed output starts and again at the end.
    pub fn clear(&mut self) {
        if let Some(bar) = self.bar.take() {
            bar.finish_and_clear();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.clear();
    }
}
