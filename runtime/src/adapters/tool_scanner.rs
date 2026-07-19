//! Extracting tool calls from a model's streamed text. A model that emits
//! `<tool_call>{…}</tool_call>` blocks (see the tool grammar) hands its output
//! through this scanner, which passes ordinary text along and turns each complete
//! block into a [`ToolCall`], holding back a partial marker so it never leaks.

use std::collections::BTreeMap;

use kernel::capabilities::ToolCall;
use kernel::records::JsonValue;

use super::grammar::{CALL_CLOSE, CALL_OPEN};

/// Streams model text, separating plain text from `<tool_call>` blocks.
#[derive(Debug, Default)]
pub struct ToolCallScanner {
    pending: String,
    in_call: bool,
    call_body: String,
}

impl ToolCallScanner {
    /// A fresh scanner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the next piece of model output, returning the plain text now safe to
    /// emit and a completed tool call if the piece finished one.
    pub fn feed(&mut self, piece: &str) -> (String, Option<ToolCall>) {
        self.pending.push_str(piece);
        if self.in_call {
            return self.drain_call();
        }
        if let Some(open) = self.pending.find(CALL_OPEN) {
            let text = self.pending[..open].to_owned();
            self.call_body = self.pending[open + CALL_OPEN.len()..].to_owned();
            self.pending.clear();
            self.in_call = true;
            let (call_text, call) = self.drain_call();
            return (text + &call_text, call);
        }
        (self.emittable_prefix(), None)
    }

    /// Emit any buffered plain text once the stream ends. Text held inside an
    /// unfinished tool-call block is dropped.
    pub fn flush(&mut self) -> String {
        let result = if self.in_call {
            String::new()
        } else {
            std::mem::take(&mut self.pending)
        };
        self.pending.clear();
        self.call_body.clear();
        self.in_call = false;
        result
    }

    /// Consume buffered call-body text; once the closing marker arrives, parse the
    /// JSON into a call (or, if it doesn't parse, emit the raw block as text).
    fn drain_call(&mut self) -> (String, Option<ToolCall>) {
        self.call_body.push_str(&self.pending);
        self.pending.clear();
        let Some(close) = self.call_body.find(CALL_CLOSE) else {
            return (String::new(), None);
        };
        let json = self.call_body[..close].trim().to_owned();
        let remainder = self.call_body[close + CALL_CLOSE.len()..].to_owned();
        self.call_body.clear();
        self.in_call = false;
        self.pending = remainder;
        match parse_tool_call(&json) {
            Some(call) => (String::new(), Some(call)),
            None => (format!("{CALL_OPEN}{json}{CALL_CLOSE}"), None),
        }
    }

    /// The plain text safe to emit now: everything except a trailing run that
    /// could be the start of an opening marker, which is held for the next feed.
    fn emittable_prefix(&mut self) -> String {
        let max = (CALL_OPEN.len() - 1).min(self.pending.len());
        for overlap in (1..=max).rev() {
            let candidate = &CALL_OPEN[..overlap];
            if self.pending.ends_with(candidate) {
                let text = self.pending[..self.pending.len() - overlap].to_owned();
                self.pending = candidate.to_owned();
                return text;
            }
        }
        std::mem::take(&mut self.pending)
    }
}

/// Parse a tool-call JSON object (`{"name": …, "arguments": …}`) into a call, or
/// `None` if it isn't a well-formed object with a name.
fn parse_tool_call(json: &str) -> Option<ToolCall> {
    let JsonValue::Object(fields) = serde_json::from_str::<JsonValue>(json).ok()? else {
        return None;
    };
    let name = fields.get("name")?.as_str()?;
    let arguments = fields
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| JsonValue::Object(BTreeMap::new()));
    Some(ToolCall::new(name, arguments))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passes_through() {
        let mut scanner = ToolCallScanner::new();
        let (text, call) = scanner.feed("hello world");
        assert_eq!(text, "hello world");
        assert!(call.is_none());
    }

    #[test]
    fn a_complete_block_in_one_feed_yields_a_call() {
        let mut scanner = ToolCallScanner::new();
        let (text, call) =
            scanner.feed(r#"before <tool_call>{"name":"add","arguments":{"a":1}}</tool_call>"#);
        assert_eq!(text, "before ");
        let call = call.unwrap();
        assert_eq!(call.name, "add");
        assert_eq!(
            call.arguments,
            JsonValue::Object([("a".to_owned(), JsonValue::Int(1))].into_iter().collect())
        );
    }

    #[test]
    fn a_block_split_across_feeds_is_reassembled() {
        let mut scanner = ToolCallScanner::new();
        assert_eq!(scanner.feed("hi <tool").0, "hi ");
        // The partial marker is held; nothing emitted yet.
        assert_eq!(scanner.feed("_call>{\"name\":\"f\",").0, "");
        let (text, call) = scanner.feed(r#""arguments":{}}</tool_call>"#);
        assert_eq!(text, "");
        assert_eq!(call.unwrap().name, "f");
    }

    #[test]
    fn a_partial_opening_marker_is_held_back() {
        let mut scanner = ToolCallScanner::new();
        // "<tool" could be the start of "<tool_call>", so it's held, not emitted.
        let (text, call) = scanner.feed("answer <tool");
        assert_eq!(text, "answer ");
        assert!(call.is_none());
        // A non-marker continuation flushes the held text back out.
        let (text, _) = scanner.feed("box is open");
        assert_eq!(text, "<toolbox is open");
    }

    #[test]
    fn missing_arguments_default_to_an_empty_object() {
        let mut scanner = ToolCallScanner::new();
        let (_text, call) = scanner.feed(r#"<tool_call>{"name":"ping"}</tool_call>"#);
        let call = call.unwrap();
        assert_eq!(call.name, "ping");
        assert_eq!(call.arguments, JsonValue::Object(BTreeMap::new()));
    }

    #[test]
    fn a_malformed_block_is_emitted_as_raw_text() {
        let mut scanner = ToolCallScanner::new();
        let (text, call) = scanner.feed("<tool_call>not json</tool_call>");
        assert!(call.is_none());
        assert_eq!(text, "<tool_call>not json</tool_call>");
    }

    #[test]
    fn flush_emits_trailing_text_but_drops_an_open_block() {
        let mut scanner = ToolCallScanner::new();
        assert_eq!(scanner.feed("tail <tool").0, "tail ");
        // The held "<tool" flushes out as plain text at end of stream.
        assert_eq!(scanner.flush(), "<tool");

        let mut scanner = ToolCallScanner::new();
        scanner.feed(r#"<tool_call>{"name":"f""#); // block opened, not closed
        assert_eq!(scanner.flush(), "");
    }

    #[test]
    fn text_after_a_closed_block_continues() {
        let mut scanner = ToolCallScanner::new();
        let (_text, call) =
            scanner.feed(r#"<tool_call>{"name":"f","arguments":{}}</tool_call>done"#);
        assert!(call.is_some());
        // The remainder after the block is buffered; a following feed emits it.
        let (text, _) = scanner.feed("!");
        assert_eq!(text, "done!");
    }
}
