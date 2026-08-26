//! The chat pane's state: a conversation with one model, kept as turns the
//! reducer appends streamed text to. Pure; the stream itself runs in `tasks`.

use std::sync::atomic::{AtomicU64, Ordering};

use kernel::capabilities::GenerationStats;
use kernel::records::{JsonValue, ModelRecord};

use super::edit::LineEdit;
use super::event::Key;
use crate::support::payload::{self, message};

/// Who said a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    /// The person typing.
    User,
    /// The model answering.
    Model,
}

/// How a model turn ended, shown dim under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// Still streaming.
    Open,
    /// Finished; the stats line when the runtime reported any.
    Done(Option<String>),
    /// Cut short by the user.
    Stopped,
    /// The runtime gave up, with the reason.
    Failed(String),
}

/// One turn of the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// Who said it.
    pub speaker: Speaker,
    /// What was said so far.
    pub text: String,
    /// How it ended, for a model turn; a user turn is always done.
    pub ending: Ending,
}

/// Asks are numbered across every pane of the run, so a reply that outlives
/// the pane it was asked in can never match an ask in the next one.
static NEXT_ASK: AtomicU64 = AtomicU64::new(1);

/// Where the transcript is read from: the newest text, or a line held
/// still while more streams in below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The bottom, moving with every new line.
    Follow,
    /// A first line counted from the top, clamped to what the drawer last
    /// measured.
    Held(usize),
}

/// The chat pane.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatPane {
    /// The model being talked to.
    pub record: ModelRecord,
    /// The conversation, oldest first.
    pub turns: Vec<Turn>,
    /// The prompt being typed.
    pub input: LineEdit,
    /// Where the transcript is read from.
    pub view: View,
    /// The furthest line the transcript can start on and still fill the
    /// pane, as the drawer last measured it; only the drawer knows the width.
    furthest: usize,
    /// The ask the open model turn belongs to, so a reply that was stopped
    /// can't write into the turn that came after it; zero before the first.
    generation: u64,
}

impl ChatPane {
    /// An empty conversation with `record`.
    pub fn open(record: ModelRecord) -> Self {
        Self {
            record,
            turns: Vec::new(),
            input: LineEdit::default(),
            view: View::Follow,
            furthest: 0,
            generation: 0,
        }
    }

    /// Whether a reply is still streaming in.
    pub fn streaming(&self) -> bool {
        self.turns
            .last()
            .is_some_and(|turn| turn.ending == Ending::Open)
    }

    /// Whether a reply was asked for and nothing has come back yet.
    pub fn waiting(&self) -> bool {
        self.turns
            .last()
            .is_some_and(|turn| turn.ending == Ending::Open && turn.text.is_empty())
    }

    /// Edit the prompt with `key`.
    pub fn edit(&mut self, key: Key) {
        self.input.apply(key);
    }

    /// Send what was typed: the user turn joins the transcript, an open model
    /// turn waits for the reply, and the payload for the kernel comes back
    /// with the generation it belongs to. Nothing happens on a blank prompt
    /// or while a reply streams.
    pub fn submit(&mut self) -> Option<(JsonValue, u64)> {
        let prompt = self.input.trimmed().to_owned();
        if prompt.is_empty() || self.streaming() {
            return None;
        }
        self.turns.push(Turn {
            speaker: Speaker::User,
            text: prompt,
            ending: Ending::Done(None),
        });
        self.input.clear();
        let payload = JsonValue::Object(payload::chat(self.history(), None));
        self.turns.push(Turn {
            speaker: Speaker::Model,
            text: String::new(),
            ending: Ending::Open,
        });
        self.generation = NEXT_ASK.fetch_add(1, Ordering::Relaxed);
        self.view = View::Follow;
        Some((payload, self.generation))
    }

    /// The conversation so far as chat messages, the shape `hedos chat`
    /// sends; a model turn that never said anything is left out.
    fn history(&self) -> Vec<JsonValue> {
        self.turns
            .iter()
            .filter(|turn| turn.speaker == Speaker::User || !turn.text.is_empty())
            .map(|turn| {
                let role = match turn.speaker {
                    Speaker::User => "user",
                    Speaker::Model => "assistant",
                };
                message(role, &turn.text)
            })
            .collect()
    }

    /// Streamed text for `generation`; ignored once that ask is over.
    pub fn text(&mut self, generation: u64, chunk: &str) -> bool {
        let Some(turn) = self.open_turn(generation) else {
            return false;
        };
        turn.text.push_str(chunk);
        true
    }

    /// The reply for `generation` ended, with the runtime's stats if any.
    pub fn done(&mut self, generation: u64, stats: Option<GenerationStats>) -> bool {
        let Some(turn) = self.open_turn(generation) else {
            return false;
        };
        turn.ending = Ending::Done(stats.as_ref().and_then(stats_line));
        true
    }

    /// The reply for `generation` failed; what streamed so far stands.
    pub fn failed(&mut self, generation: u64, reason: String) -> bool {
        let Some(turn) = self.open_turn(generation) else {
            return false;
        };
        turn.ending = Ending::Failed(reason);
        true
    }

    /// Stop the reply in flight; what streamed so far stands.
    pub fn stop(&mut self) {
        if let Some(turn) = self.open_turn(self.generation) {
            turn.ending = Ending::Stopped;
        }
    }

    fn open_turn(&mut self, generation: u64) -> Option<&mut Turn> {
        if generation != self.generation {
            return None;
        }
        self.turns
            .last_mut()
            .filter(|turn| turn.ending == Ending::Open)
    }

    /// The first line shown, given what the drawer last measured.
    pub fn first_line(&self) -> usize {
        match self.view {
            View::Follow => self.furthest,
            View::Held(first) => first.min(self.furthest),
        }
    }

    /// Take the drawer's measurement of how far the transcript can scroll; a
    /// held view that reached the bottom follows again.
    pub fn measured(&mut self, furthest: usize) {
        self.furthest = furthest;
        if self.view != View::Follow {
            self.hold(self.first_line());
        }
    }

    /// Hold the transcript at `first`, or follow when that is the bottom.
    fn hold(&mut self, first: usize) {
        self.view = if first >= self.furthest {
            View::Follow
        } else {
            View::Held(first)
        };
    }

    /// Hold the transcript `lines` further up.
    pub fn scroll_up(&mut self, lines: usize) {
        self.hold(self.first_line().saturating_sub(lines));
    }

    /// Let the transcript `lines` back down; at the bottom it follows again.
    pub fn scroll_down(&mut self, lines: usize) {
        self.hold(self.first_line().saturating_add(lines));
    }

    /// Show the start of the transcript.
    pub fn scroll_to_top(&mut self) {
        self.hold(0);
    }

    /// Follow the newest text again.
    pub fn scroll_to_bottom(&mut self) {
        self.view = View::Follow;
    }
}

/// `142 tokens · 38 tok/s · first in 0.4s`, from whatever was reported.
fn stats_line(stats: &GenerationStats) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(tokens) = stats.completion_tokens {
        let estimated = if stats.token_counts_estimated {
            "~"
        } else {
            ""
        };
        parts.push(format!("{estimated}{tokens} tokens"));
        if let Some(ms) = stats.duration_ms.filter(|ms| *ms > 0) {
            parts.push(format!("{:.0} tok/s", tokens as f64 * 1000.0 / ms as f64));
        }
    }
    if let Some(ms) = stats.ttft_ms {
        parts.push(format!("first in {:.1}s", ms as f64 / 1000.0));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Capability, Modality, ModelSource, SourceKind};

    fn pane() -> ChatPane {
        ChatPane::open(ModelRecord::new(
            "m",
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::ollama(), "m"),
        ))
    }

    fn type_in(pane: &mut ChatPane, text: &str) {
        for c in text.chars() {
            pane.edit(Key::Char(c));
        }
    }

    /// The contents of the messages in a chat payload.
    fn contents(payload: &JsonValue) -> Vec<&str> {
        payload
            .as_object()
            .and_then(|object| object.get("messages"))
            .and_then(JsonValue::as_array)
            .expect("messages")
            .iter()
            .filter_map(|message| message.as_object()?.get("content")?.as_str())
            .collect()
    }

    #[test]
    fn a_blank_prompt_or_a_streaming_reply_blocks_a_send() {
        let mut pane = pane();
        type_in(&mut pane, "  ");
        assert_eq!(pane.submit(), None);
        type_in(&mut pane, "hi");
        let (_, generation) = pane.submit().expect("sent");
        assert!(generation > 0);
        assert!(pane.streaming());
        type_in(&mut pane, "again");
        assert_eq!(pane.submit(), None);
        assert_eq!(pane.input.as_str(), "again");
    }

    #[test]
    fn the_history_carries_every_said_turn() {
        let mut pane = pane();
        type_in(&mut pane, "one");
        let (payload, generation) = pane.submit().expect("sent");
        assert_eq!(contents(&payload).len(), 1);
        assert!(pane.text(generation, "an"));
        assert!(pane.text(generation, "swer"));
        assert!(pane.done(generation, None));
        assert!(!pane.streaming());
        type_in(&mut pane, "two");
        let (payload, _) = pane.submit().expect("sent");
        assert_eq!(contents(&payload), ["one", "answer", "two"]);
    }

    #[test]
    fn a_stopped_reply_ignores_late_chunks_and_drops_out_of_history() {
        let mut pane = pane();
        type_in(&mut pane, "hi");
        let (_, first) = pane.submit().expect("sent");
        pane.stop();
        assert!(!pane.streaming());
        assert!(!pane.text(first, "late"));
        assert!(!pane.done(first, None));
        assert_eq!(
            pane.turns.last().map(|turn| &turn.ending),
            Some(&Ending::Stopped)
        );
        type_in(&mut pane, "again");
        let (payload, second) = pane.submit().expect("sent");
        assert!(second > first);
        assert_eq!(contents(&payload).len(), 2);
        assert!(!pane.text(first, "later still"));
    }

    #[test]
    fn a_failure_keeps_the_partial_text_and_the_reason() {
        let mut pane = pane();
        type_in(&mut pane, "hi");
        let (_, generation) = pane.submit().expect("sent");
        pane.text(generation, "par");
        pane.failed(generation, "sidecar died".to_owned());
        let turn = pane.turns.last().expect("a turn");
        assert_eq!(turn.text, "par");
        assert_eq!(turn.ending, Ending::Failed("sidecar died".to_owned()));
    }

    #[test]
    fn a_held_view_stays_put_while_text_streams_and_follows_from_the_bottom() {
        let mut pane = pane();
        type_in(&mut pane, "hi");
        let (_, generation) = pane.submit().expect("sent");
        pane.measured(40);
        assert_eq!(pane.first_line(), 40);
        pane.scroll_up(5);
        assert_eq!(pane.view, View::Held(35));
        pane.text(generation, "more");
        pane.measured(60);
        assert_eq!(pane.first_line(), 35);
        pane.scroll_down(30);
        assert_eq!(pane.view, View::Follow);
        pane.scroll_to_top();
        assert_eq!(pane.first_line(), 0);
        pane.scroll_up(3);
        assert_eq!(pane.view, View::Held(0));
        pane.measured(0);
        assert_eq!(pane.view, View::Follow);
        pane.measured(9);
        pane.scroll_up(4);
        pane.measured(2);
        assert_eq!(pane.view, View::Follow);
        type_in(&mut pane, "again");
        pane.done(generation, None);
        pane.submit();
        assert_eq!(pane.view, View::Follow);
    }

    #[test]
    fn stats_read_as_one_dim_line() {
        let stats = GenerationStats {
            completion_tokens: Some(120),
            duration_ms: Some(3000),
            ttft_ms: Some(420),
            ..GenerationStats::default()
        };
        assert_eq!(
            stats_line(&stats).as_deref(),
            Some("120 tokens · 40 tok/s · first in 0.4s")
        );
        assert_eq!(stats_line(&GenerationStats::default()), None);
    }
}
