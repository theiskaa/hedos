//! Streaming stop-sequence detection. Text is emitted as it arrives, except for
//! a trailing suffix that could be the start of a stop sequence, which is held
//! back until the next chunk resolves it.

use crate::capabilities::held_suffix_len;
use crate::records::JsonValue;

/// A stop-sequence matcher fed text incrementally.
#[derive(Debug, Clone)]
pub struct StopMatcher {
    stops: Vec<String>,
    buffer: String,
    stopped: bool,
}

impl StopMatcher {
    /// Create a matcher for the given stop sequences. Empty strings are ignored.
    pub fn new(stops: Vec<String>) -> Self {
        Self {
            stops: stops.into_iter().filter(|stop| !stop.is_empty()).collect(),
            buffer: String::new(),
            stopped: false,
        }
    }

    /// Whether any stop sequence is configured.
    pub fn is_active(&self) -> bool {
        !self.stops.is_empty()
    }

    /// Whether a stop sequence has been reached.
    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Feed a chunk and return the text safe to emit now. When a stop sequence is
    /// reached, the text up to it is emitted, the matcher latches `stopped`, and
    /// further feeds return nothing. With no stops configured, the chunk passes
    /// straight through.
    pub fn feed(&mut self, chunk: &str) -> String {
        if !self.is_active() {
            return chunk.to_owned();
        }
        if self.stopped {
            return String::new();
        }
        self.buffer.push_str(chunk);
        if let Some(position) = self.earliest_stop() {
            let emit = self.buffer[..position].to_owned();
            self.buffer.clear();
            self.stopped = true;
            return emit;
        }
        let held = held_suffix_len(&self.buffer, &self.stops);
        let split = self.buffer.len() - held;
        let emit = self.buffer[..split].to_owned();
        self.buffer.drain(..split);
        emit
    }

    /// Emit any buffered text that was held back. Returns nothing once stopped.
    pub fn flush(&mut self) -> String {
        if self.stopped {
            return String::new();
        }
        std::mem::take(&mut self.buffer)
    }

    fn earliest_stop(&self) -> Option<usize> {
        self.stops
            .iter()
            .filter_map(|stop| self.buffer.find(stop.as_str()))
            .min()
    }
}

/// Extract stop sequences from a parameter value: a single string, or an array
/// of strings. Anything else yields no stops.
pub fn stop_strings(value: Option<&JsonValue>) -> Vec<String> {
    match value {
        Some(JsonValue::String(single)) => vec![single.clone()],
        Some(JsonValue::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}
