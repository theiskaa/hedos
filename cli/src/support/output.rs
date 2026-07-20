//! Output helpers: human-readable lines on stdout, notices on stderr, streamed
//! tokens without a trailing newline, and machine-readable JSON under `--json`.

use std::io::Write;

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
            println!("{text}");
        }
    }

    /// Raw output on stdout with no newline, flushed — for streamed tokens.
    pub fn raw(&self, text: &str) {
        if !self.json {
            print!("{text}");
            let _ = std::io::stdout().flush();
        }
    }

    /// A notice on stderr (status, prompts) — always shown.
    pub fn err(&self, text: &str) {
        eprintln!("{text}");
    }

    /// A JSON document on stdout (pretty-printed). A no-op outside JSON mode.
    pub fn json(&self, value: &serde_json::Value) {
        if self.json {
            match serde_json::to_string_pretty(value) {
                Ok(text) => println!("{text}"),
                Err(error) => eprintln!("{error}"),
            }
        }
    }
}
