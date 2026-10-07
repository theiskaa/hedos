//! The chat pane's state: a conversation with one model, kept as turns the
//! reducer appends streamed text to. Pure; the stream itself runs in `tasks`.
//!
//! A judge takes no prose, so its pane holds a draft instead of a prompt: a
//! situation, a question, its kind, and the options or levels it may be
//! answered with. Each ask stands alone, and its answer is a distribution.

use std::sync::atomic::{AtomicU64, Ordering};

use kernel::capabilities::GenerationStats;
use kernel::records::{JsonValue, ModelRecord};

use super::edit::LineEdit;
use super::event::Key;
use crate::support::judge::{self, Kind, Question};
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
    /// Finished, with the runtime's stats when it reported any.
    Done(Option<GenerationStats>),
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
    /// When it was said or asked for, in milliseconds since the screen
    /// opened.
    pub at_ms: u64,
    /// When it was said, in Unix milliseconds, for the time beside it.
    pub wall_ms: i64,
    /// When the first of a reply's text arrived.
    pub first_token_ms: Option<u64>,
    /// When a reply ended, however it ended.
    pub ended_ms: Option<u64>,
    /// Whether the model was cold when asked, so the wait reads as loading
    /// it into memory rather than thinking.
    pub cold: bool,
    /// What a judge was asked, on both the ask and its answer, so the
    /// answer can be read against the question that produced it.
    pub asked: Option<Box<Asked>>,
}

/// What one ask of a judge carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    /// What is being judged; empty when the question stands on its own.
    pub situation: String,
    /// The question, its kind, and its options or levels.
    pub question: Question,
}

impl Turn {
    fn new(speaker: Speaker, text: String, ending: Ending, at_ms: u64, wall_ms: i64) -> Self {
        Self {
            speaker,
            text,
            ending,
            at_ms,
            wall_ms,
            first_token_ms: None,
            ended_ms: None,
            cold: false,
            asked: None,
        }
    }

    /// The reply's speed in tokens a second, and whether the count behind it
    /// was estimated: from the runtime's figures when it reported them, else
    /// from the text, about four characters a token, over the time it took
    /// to arrive.
    pub fn rate(&self) -> Option<(f64, bool)> {
        if let Ending::Done(Some(stats)) = &self.ending
            && let Some(tokens) = stats.completion_tokens
            && let Some(ms) = stats.eval_ms.or(stats.duration_ms).filter(|ms| *ms > 0)
        {
            return Some((
                tokens as f64 * 1000.0 / ms as f64,
                stats.token_counts_estimated,
            ));
        }
        let first = self.first_token_ms?;
        let end = self.ended_ms?;
        let ms = end.checked_sub(first).filter(|ms| *ms > 0)?;
        Some((
            estimated_tokens(&self.text) as f64 * 1000.0 / ms as f64,
            true,
        ))
    }

    /// How long the reply took to its first text, in milliseconds.
    pub fn first_token_after(&self) -> Option<u64> {
        if let Ending::Done(Some(stats)) = &self.ending
            && let Some(ms) = stats.ttft_ms
        {
            return u64::try_from(ms).ok();
        }
        self.first_token_ms
            .map(|first| first.saturating_sub(self.at_ms))
    }

    /// The tokens a finished reply came to, and whether that was counted
    /// from its text.
    pub fn tokens(&self) -> (u64, bool) {
        match &self.ending {
            Ending::Done(Some(stats)) if stats.completion_tokens.is_some() => (
                stats.completion_tokens.unwrap_or(0).max(0) as u64,
                stats.token_counts_estimated,
            ),
            _ => (estimated_tokens(&self.text), true),
        }
    }
}

/// About how many tokens `text` is: four characters to a token.
fn estimated_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

/// Three things to ask a model that has not been asked anything yet.
pub const SUGGESTIONS: [&str; 3] = [
    "explain what you can do in two lines",
    "what is a kv cache, briefly?",
    "write a haiku about a koala on a shelf",
];

/// The kinds of question a judge takes, in the order the composer offers
/// them.
pub const KINDS: [Kind; 3] = [Kind::Choice, Kind::Score, Kind::Noul];

/// The fields of the judge's composer, in the order `tab` moves through
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// Which kind of question.
    Kind,
    /// What is being judged.
    Situation,
    /// What is asked about it.
    Question,
    /// A choice's options or a score's levels, one at a time.
    Options,
}

/// What the judge's composer holds while a question is put together; an
/// ask empties it, keeping only the kind.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Draft {
    /// Which of [`KINDS`] is asked.
    pub kind: usize,
    /// Which field takes the keys.
    pub field: Option<Field>,
    /// What is being judged.
    pub situation: LineEdit,
    /// The question, or a noul's statement.
    pub question: LineEdit,
    /// The option or level being typed.
    pub option: LineEdit,
    /// The options or levels given so far, as they were typed; each is
    /// read under the kind asked, so changing the kind reads them again.
    pub options: Vec<String>,
}

impl Draft {
    /// The kind of question asked.
    pub fn kind(&self) -> Kind {
        KINDS[self.kind % KINDS.len()]
    }

    /// The field taking the keys; the situation until another is chosen.
    pub fn field(&self) -> Field {
        self.field.unwrap_or(Field::Situation)
    }

    /// The fields shown for the kind asked: a noul has no options.
    pub fn fields(&self) -> &'static [Field] {
        match self.kind() {
            Kind::Noul => &[Field::Kind, Field::Situation, Field::Question],
            _ => &[
                Field::Kind,
                Field::Situation,
                Field::Question,
                Field::Options,
            ],
        }
    }

    /// Move `delta` fields along, wrapping.
    pub fn step(&mut self, delta: isize) {
        let fields = self.fields();
        let at = fields
            .iter()
            .position(|field| *field == self.field())
            .unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(fields.len() as isize) as usize;
        self.field = Some(fields[next]);
    }

    /// The options or levels given so far as `(label, meaning)`, read
    /// under the kind asked.
    pub fn entries(&self) -> Vec<(String, String)> {
        self.options
            .iter()
            .map(|line| {
                let (label, meaning) = judge::split_entry(self.kind(), line);
                (label.to_owned(), meaning.to_owned())
            })
            .collect()
    }

    /// Move `delta` kinds along, wrapping.
    pub fn cycle_kind(&mut self, delta: isize) {
        self.kind = (self.kind as isize + delta).rem_euclid(KINDS.len() as isize) as usize;
    }

    /// Edit the field taking the keys with `key`. On the kind, left, right
    /// and space change it; backspace in an empty option field takes the
    /// last option back to edit.
    pub fn edit(&mut self, key: Key) {
        use super::event::Edit;
        match (self.field(), key) {
            (Field::Kind, Key::Edit(Edit::Left)) => self.cycle_kind(-1),
            (Field::Kind, Key::Edit(Edit::Right) | Key::Char(' ')) => self.cycle_kind(1),
            (Field::Kind, _) => {}
            (Field::Situation, key) => {
                self.situation.apply(key);
            }
            (Field::Question, key) => {
                self.question.apply(key);
            }
            (Field::Options, Key::Backspace) if self.option.is_empty() => {
                if let Some(line) = self.options.pop() {
                    for c in line.chars() {
                        self.option.apply(Key::Char(c));
                    }
                }
            }
            (Field::Options, key) => {
                self.option.apply(key);
            }
        }
    }

    /// Add the option being typed, or say why it can't be.
    fn add_option(&mut self) -> Result<(), String> {
        let noun = self.noun();
        let line = self.option.trimmed().to_owned();
        let (label, meaning) = judge::split_entry(self.kind(), &line);
        if label.is_empty() {
            return Err(format!("a {noun} needs a label before the colon"));
        }
        if self.entries().iter().any(|(existing, _)| existing == label) {
            return Err(format!("\"{label}\" is already a {noun}"));
        }
        let line = if meaning.is_empty() {
            label.to_owned()
        } else {
            format!("{label}: {meaning}")
        };
        self.options.push(line);
        self.option.clear();
        Ok(())
    }

    /// `option` for a choice, `level` for a score.
    pub fn noun(&self) -> &'static str {
        match self.kind() {
            Kind::Score => "level",
            _ => "option",
        }
    }

    /// The question the draft makes, or why it makes none yet.
    fn question(&self) -> Result<Question, String> {
        let instructions = self.question.trimmed().to_owned();
        if instructions.is_empty() {
            return Err(match self.kind() {
                Kind::Noul => "write the statement to weigh".to_owned(),
                _ => "write the question to ask".to_owned(),
            });
        }
        let kind = self.kind();
        let criteria = match kind {
            Kind::Noul => Vec::new(),
            _ if self.options.len() < 2 => {
                return Err(format!(
                    "two {}s at least: type one and press enter",
                    self.noun()
                ));
            }
            _ => {
                let entries = self.entries();
                if let Some((label, _)) = entries
                    .iter()
                    .enumerate()
                    .find(|(index, (label, _))| {
                        entries[..*index]
                            .iter()
                            .any(|(earlier, _)| earlier == label)
                    })
                    .map(|(_, entry)| entry)
                {
                    return Err(format!("\"{label}\" is listed twice"));
                }
                entries
            }
        };
        Ok(Question {
            id: "q1".to_owned(),
            kind,
            instructions,
            criteria,
        })
    }
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
    /// When the pane opened, on the animation clock, for its arrival.
    pub opened_at: u64,
    /// Which suggestion `tab` puts in the box next.
    suggestion: usize,
    /// The judge's composer, when the model is a judge.
    pub draft: Option<Box<Draft>>,
}

/// What enter did in the judge's composer.
#[derive(Debug, Clone, PartialEq)]
pub enum Judged {
    /// Moved to the next field, or added an option.
    Moved,
    /// Asked: the payload for the kernel and the generation it belongs to.
    Asked(JsonValue, u64),
    /// Nothing, and why.
    Refused(String),
}

impl ChatPane {
    /// An empty conversation with `record`.
    pub fn open(record: ModelRecord) -> Self {
        Self {
            turns: Vec::new(),
            input: LineEdit::default(),
            view: View::Follow,
            furthest: 0,
            generation: 0,
            opened_at: 0,
            suggestion: 0,
            draft: judge::is_judge(&record).then(Box::default),
            record,
        }
    }

    /// Whether leaving the pane would lose anything: a turn, a typed
    /// prompt, or any part of a judge's draft.
    pub fn worth_keeping(&self) -> bool {
        !self.turns.is_empty()
            || !self.input.is_empty()
            || self.draft.as_deref().is_some_and(|draft| {
                !(draft.situation.is_empty()
                    && draft.question.is_empty()
                    && draft.option.is_empty()
                    && draft.options.is_empty())
            })
    }

    /// Whether the model is a judge, asked typed questions rather than
    /// talked to.
    pub fn judging(&self) -> bool {
        self.draft.is_some()
    }

    /// The same pane, opened at `at` on the animation clock.
    pub fn opened(mut self, at: u64) -> Self {
        self.opened_at = at;
        self
    }

    /// Move the judge's composer `delta` fields along.
    pub fn next_field(&mut self, delta: isize) {
        if let Some(draft) = self.draft.as_mut() {
            draft.step(delta);
        }
    }

    /// Put a suggestion in the box: the next one each time, but only over an
    /// empty box or a suggestion, never over what was typed.
    pub fn suggest(&mut self) -> bool {
        let current = self.input.as_str();
        if !current.is_empty() && !SUGGESTIONS.contains(&current) {
            return false;
        }
        let index = match SUGGESTIONS
            .iter()
            .position(|suggestion| *suggestion == current)
        {
            Some(index) => (index + 1) % SUGGESTIONS.len(),
            None => self.suggestion % SUGGESTIONS.len(),
        };
        self.suggestion = index;
        self.input.clear();
        for c in SUGGESTIONS[index].chars() {
            self.input.apply(Key::Char(c));
        }
        true
    }

    /// Which suggestion `tab` puts in the box next.
    pub fn suggestion(&self) -> usize {
        self.suggestion % SUGGESTIONS.len()
    }

    /// Start the conversation over with the same model; refused while a
    /// reply streams. The ask counter is shared by every pane, so a late
    /// chunk from before can never land in what comes after.
    pub fn clear(&mut self) -> bool {
        if self.streaming() {
            return false;
        }
        self.turns.clear();
        self.view = View::Follow;
        self.furthest = 0;
        true
    }

    /// How much of the model's context the conversation fills: what the
    /// last reply's runtime counted when it did, else the text, about four
    /// characters a token; and whether that was estimated.
    pub fn context_used(&self) -> (u64, bool) {
        let counted = self.turns.iter().rev().find_map(|turn| match &turn.ending {
            Ending::Done(Some(stats)) if turn.speaker == Speaker::Model => {
                Some((stats.prompt_tokens?, stats.completion_tokens?))
            }
            _ => None,
        });
        match counted {
            Some((prompt, completion)) => ((prompt + completion).max(0) as u64, false),
            None => (
                self.turns
                    .iter()
                    .map(|turn| estimated_tokens(&turn.text))
                    .sum(),
                true,
            ),
        }
    }

    /// The streaming reply's speed so far, in estimated tokens a second.
    pub fn live_rate(&self, now_ms: u64) -> Option<f64> {
        let turn = self
            .turns
            .last()
            .filter(|turn| turn.ending == Ending::Open)?;
        let first = turn.first_token_ms?;
        let ms = now_ms.checked_sub(first).filter(|ms| *ms > 0)?;
        Some(estimated_tokens(&turn.text) as f64 * 1000.0 / ms as f64)
    }

    /// When the conversation started, in Unix milliseconds.
    pub fn since_wall_ms(&self) -> Option<i64> {
        self.turns.first().map(|turn| turn.wall_ms)
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

    /// Edit the prompt with `key`, or the judge's field taking the keys.
    pub fn edit(&mut self, key: Key) {
        match self.draft.as_mut() {
            Some(draft) => draft.edit(key),
            None => {
                self.input.apply(key);
            }
        }
    }

    /// Enter in the judge's composer at `now_ms` (`wall_ms` on the wall):
    /// on the kind or the situation it moves on; on the question it moves
    /// to the options, or asks a noul; on the options it adds the one typed,
    /// or asks once nothing is. `warm` says whether the model is in memory.
    pub fn judge_enter(&mut self, now_ms: u64, wall_ms: i64, warm: bool) -> Judged {
        let streaming = self.streaming();
        let Some(draft) = self.draft.as_mut() else {
            return Judged::Moved;
        };
        let ask = match draft.field() {
            Field::Kind | Field::Situation => false,
            Field::Question if draft.question.trimmed().is_empty() => {
                return Judged::Refused(match draft.kind() {
                    Kind::Noul => "write the statement to weigh".to_owned(),
                    _ => "write the question to ask".to_owned(),
                });
            }
            Field::Question => draft.kind() == Kind::Noul,
            Field::Options if !draft.option.trimmed().is_empty() => {
                return match draft.add_option() {
                    Ok(()) => Judged::Moved,
                    Err(reason) => Judged::Refused(reason),
                };
            }
            Field::Options => true,
        };
        if !ask {
            draft.step(1);
            return Judged::Moved;
        }
        if streaming {
            return Judged::Refused("the judge is still weighing the last ask".to_owned());
        }
        let question = match draft.question() {
            Ok(question) => question,
            Err(reason) => {
                if draft.question.trimmed().is_empty() {
                    draft.field = Some(Field::Question);
                }
                return Judged::Refused(reason);
            }
        };
        let situation = draft.situation.trimmed().to_owned();
        draft.situation.clear();
        draft.question.clear();
        draft.option.clear();
        draft.options.clear();
        draft.field = Some(Field::Situation);
        let text = judge::request_text(&situation, std::slice::from_ref(&question));
        let payload = JsonValue::Object(payload::chat(
            vec![payload::user_message_with_images(&text, Vec::new())],
            None,
        ));
        let asked = Box::new(Asked {
            situation,
            question,
        });
        let mut ask = Turn::new(
            Speaker::User,
            String::new(),
            Ending::Done(None),
            now_ms,
            wall_ms,
        );
        ask.asked = Some(asked.clone());
        self.turns.push(ask);
        let mut reply = Turn::new(Speaker::Model, String::new(), Ending::Open, now_ms, wall_ms);
        reply.cold = !warm;
        reply.asked = Some(asked);
        self.turns.push(reply);
        self.generation = NEXT_ASK.fetch_add(1, Ordering::Relaxed);
        self.view = View::Follow;
        Judged::Asked(payload, self.generation)
    }

    /// Send what was typed at `now_ms` (`wall_ms` on the clock on the wall):
    /// the user turn joins the transcript, an open model turn waits for the
    /// reply, and the payload for the kernel comes back with the generation
    /// it belongs to. `warm` says whether the model is already in memory.
    /// Nothing happens on a blank prompt or while a reply streams.
    pub fn submit(&mut self, now_ms: u64, wall_ms: i64, warm: bool) -> Option<(JsonValue, u64)> {
        let prompt = self.input.trimmed().to_owned();
        if prompt.is_empty() || self.streaming() {
            return None;
        }
        self.turns.push(Turn::new(
            Speaker::User,
            prompt,
            Ending::Done(None),
            now_ms,
            wall_ms,
        ));
        self.input.clear();
        let payload = JsonValue::Object(payload::chat(self.history(), None));
        let mut reply = Turn::new(Speaker::Model, String::new(), Ending::Open, now_ms, wall_ms);
        reply.cold = !warm;
        self.turns.push(reply);
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

    /// Streamed text for `generation`, arriving at `now_ms`; ignored once
    /// that ask is over.
    pub fn append(&mut self, generation: u64, chunk: &str, now_ms: u64) -> bool {
        let Some(turn) = self.open_turn(generation) else {
            return false;
        };
        if turn.first_token_ms.is_none() && !chunk.is_empty() {
            turn.first_token_ms = Some(now_ms);
        }
        turn.text.push_str(chunk);
        true
    }

    /// The reply for `generation` ended at `now_ms`, with the runtime's stats
    /// if any.
    pub fn done(&mut self, generation: u64, stats: Option<GenerationStats>, now_ms: u64) -> bool {
        let Some(turn) = self.open_turn(generation) else {
            return false;
        };
        turn.ending = Ending::Done(stats);
        turn.ended_ms = Some(now_ms);
        true
    }

    /// The reply for `generation` failed at `now_ms`; what streamed so far
    /// stands.
    pub fn failed(&mut self, generation: u64, reason: String, now_ms: u64) -> bool {
        let Some(turn) = self.open_turn(generation) else {
            return false;
        };
        turn.ending = Ending::Failed(reason);
        turn.ended_ms = Some(now_ms);
        true
    }

    /// Stop the reply in flight at `now_ms`; what streamed so far stands.
    pub fn stop(&mut self, now_ms: u64) {
        if let Some(turn) = self.open_turn(self.generation) {
            turn.ending = Ending::Stopped;
            turn.ended_ms = Some(now_ms);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::testing::record;

    fn pane() -> ChatPane {
        ChatPane::open(record("m"))
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
        assert_eq!(pane.submit(0, 0, true), None);
        type_in(&mut pane, "hi");
        let (_, generation) = pane.submit(0, 0, true).expect("sent");
        assert!(generation > 0);
        assert!(pane.streaming());
        type_in(&mut pane, "again");
        assert_eq!(pane.submit(0, 0, true), None);
        assert_eq!(pane.input.as_str(), "again");
    }

    #[test]
    fn the_history_carries_every_said_turn() {
        let mut pane = pane();
        type_in(&mut pane, "one");
        let (payload, generation) = pane.submit(0, 0, true).expect("sent");
        assert_eq!(contents(&payload).len(), 1);
        assert!(pane.append(generation, "an", 0));
        assert!(pane.append(generation, "swer", 0));
        assert!(pane.done(generation, None, 0));
        assert!(!pane.streaming());
        type_in(&mut pane, "two");
        let (payload, _) = pane.submit(0, 0, true).expect("sent");
        assert_eq!(contents(&payload), ["one", "answer", "two"]);
    }

    #[test]
    fn a_stopped_reply_ignores_late_chunks_and_drops_out_of_history() {
        let mut pane = pane();
        type_in(&mut pane, "hi");
        let (_, first) = pane.submit(0, 0, true).expect("sent");
        pane.stop(0);
        assert!(!pane.streaming());
        assert!(!pane.append(first, "late", 0));
        assert!(!pane.done(first, None, 0));
        assert_eq!(
            pane.turns.last().map(|turn| &turn.ending),
            Some(&Ending::Stopped)
        );
        type_in(&mut pane, "again");
        let (payload, second) = pane.submit(0, 0, true).expect("sent");
        assert!(second > first);
        assert_eq!(contents(&payload).len(), 2);
        assert!(!pane.append(first, "later still", 0));
    }

    #[test]
    fn waiting_ends_with_the_first_token() {
        let mut pane = pane();
        type_in(&mut pane, "hi");
        let (_, generation) = pane.submit(0, 0, true).expect("sent");
        assert!(pane.waiting());
        pane.append(generation, "a", 0);
        assert!(!pane.waiting() && pane.streaming());
    }

    #[test]
    fn a_stopped_reply_with_text_stays_in_history() {
        let mut pane = pane();
        type_in(&mut pane, "hi");
        let (_, generation) = pane.submit(0, 0, true).expect("sent");
        pane.append(generation, "part", 0);
        pane.stop(0);
        type_in(&mut pane, "more");
        let (payload, _) = pane.submit(0, 0, true).expect("sent");
        assert_eq!(contents(&payload), ["hi", "part", "more"]);
    }

    #[test]
    fn a_failure_keeps_the_partial_text_and_the_reason() {
        let mut pane = pane();
        type_in(&mut pane, "hi");
        let (_, generation) = pane.submit(0, 0, true).expect("sent");
        pane.append(generation, "par", 0);
        pane.failed(generation, "sidecar died".to_owned(), 0);
        let turn = pane.turns.last().expect("a turn");
        assert_eq!(turn.text, "par");
        assert_eq!(turn.ending, Ending::Failed("sidecar died".to_owned()));
    }

    #[test]
    fn a_held_view_stays_put_while_text_streams_and_follows_from_the_bottom() {
        let mut pane = pane();
        type_in(&mut pane, "hi");
        let (_, generation) = pane.submit(0, 0, true).expect("sent");
        pane.measured(40);
        assert_eq!(pane.first_line(), 40);
        pane.scroll_up(5);
        assert_eq!(pane.view, View::Held(35));
        pane.append(generation, "more", 0);
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
        pane.done(generation, None, 0);
        pane.submit(0, 0, true);
        assert_eq!(pane.view, View::Follow);
    }

    #[test]
    fn a_turn_keeps_when_it_was_asked_answered_and_ended() {
        let mut pane = pane();
        type_in(&mut pane, "hi");
        let (_, generation) = pane.submit(1_000, 1_700_000_000_000, false).expect("sent");
        let reply = pane.turns.last().expect("a reply");
        assert!(reply.cold && reply.at_ms == 1_000);
        assert_eq!(pane.turns[0].wall_ms, 1_700_000_000_000);
        pane.append(generation, "", 1_200);
        assert_eq!(
            pane.turns[1].first_token_ms, None,
            "an empty chunk is not text"
        );
        pane.append(generation, "abcdefgh", 1_400);
        pane.append(generation, "ijklmnop", 1_900);
        assert_eq!(pane.live_rate(2_400), Some(4.0));
        pane.done(generation, None, 2_400);
        let reply = &pane.turns[1];
        assert_eq!(reply.first_token_after(), Some(400));
        assert_eq!(
            reply.rate(),
            Some((4.0, true)),
            "4 tokens over a second, estimated"
        );
        assert_eq!(reply.tokens(), (4, true));
        assert_eq!(pane.context_used(), (5, true));
        assert_eq!(pane.since_wall_ms(), Some(1_700_000_000_000));
    }

    #[test]
    fn the_runtimes_figures_win_over_the_estimate() {
        let mut pane = pane();
        type_in(&mut pane, "hi");
        let (_, generation) = pane.submit(0, 0, true).expect("sent");
        pane.append(generation, "words", 100);
        let stats = GenerationStats {
            prompt_tokens: Some(20),
            completion_tokens: Some(30),
            eval_ms: Some(600),
            ttft_ms: Some(80),
            ..GenerationStats::default()
        };
        pane.done(generation, Some(stats), 900);
        let reply = &pane.turns[1];
        assert_eq!(reply.rate(), Some((50.0, false)));
        assert_eq!(reply.first_token_after(), Some(80));
        assert_eq!(reply.tokens(), (30, false));
        assert_eq!(pane.context_used(), (50, false));
    }

    #[test]
    fn clear_starts_over_but_never_mid_reply() {
        let mut pane = pane();
        type_in(&mut pane, "hi");
        let (_, generation) = pane.submit(0, 0, true).expect("sent");
        assert!(!pane.clear());
        pane.done(generation, None, 10);
        assert!(pane.clear());
        assert!(pane.turns.is_empty());
        assert!(!pane.append(generation, "late", 20));
    }

    #[test]
    fn tab_cycles_suggestions_only_over_an_empty_box_or_a_suggestion() {
        let mut pane = pane();
        assert!(pane.suggest());
        assert_eq!(pane.input.as_str(), SUGGESTIONS[0]);
        assert!(pane.suggest());
        assert_eq!(pane.input.as_str(), SUGGESTIONS[1]);
        pane.edit(Key::Char('!'));
        assert!(!pane.suggest());
        assert!(pane.input.as_str().ends_with('!'));
    }

    fn judge_pane() -> ChatPane {
        let mut record = record("judge");
        record.capabilities = vec![kernel::records::Capability::judge()];
        ChatPane::open(record)
    }

    fn enter(pane: &mut ChatPane) -> Judged {
        pane.judge_enter(0, 0, true)
    }

    #[test]
    fn a_judge_opens_on_a_draft_and_enter_walks_its_fields() {
        let mut pane = judge_pane();
        assert!(pane.judging() && !self::pane().judging());
        let draft = pane.draft.as_ref().expect("a draft");
        assert_eq!(draft.field(), Field::Situation);
        type_in(&mut pane, "the order arrived broken");
        assert_eq!(enter(&mut pane), Judged::Moved);
        assert!(matches!(enter(&mut pane), Judged::Refused(reason) if reason.contains("question")));
        type_in(&mut pane, "how should support answer?");
        assert_eq!(enter(&mut pane), Judged::Moved);
        let draft = pane.draft.as_ref().expect("a draft");
        assert_eq!(draft.field(), Field::Options);
        assert!(
            matches!(enter(&mut pane), Judged::Refused(reason) if reason.contains("two options"))
        );
        type_in(&mut pane, "refund: money back");
        assert_eq!(enter(&mut pane), Judged::Moved);
        type_in(&mut pane, "refund");
        assert!(matches!(enter(&mut pane), Judged::Refused(reason) if reason.contains("already")));
        pane.draft.as_mut().expect("a draft").option.clear();
        type_in(&mut pane, "replace");
        enter(&mut pane);
        let Judged::Asked(payload, generation) = enter(&mut pane) else {
            panic!("an ask");
        };
        assert!(generation > 0 && pane.streaming());
        let sent = contents(&payload)[0].to_owned();
        assert!(
            sent.contains("\"refund\":\"money back\",\"replace\":\"\""),
            "{sent}"
        );
        assert!(sent.contains("the order arrived broken"));
        let asked = pane.turns[1].asked.as_ref().expect("the question");
        assert_eq!(asked.question.criteria.len(), 2);
        let draft = pane.draft.as_ref().expect("a draft");
        assert!(draft.situation.is_empty() && draft.question.is_empty());
        assert!(draft.options.is_empty(), "an ask empties the fields");
        assert_eq!(draft.field(), Field::Situation);
        assert_eq!(draft.kind(), Kind::Choice);
    }

    #[test]
    fn the_kind_turns_on_its_field_and_a_noul_asks_from_the_question() {
        let mut pane = judge_pane();
        pane.next_field(-1);
        assert_eq!(pane.draft.as_ref().expect("a draft").field(), Field::Kind);
        pane.edit(Key::Char(' '));
        pane.edit(Key::Char(' '));
        let draft = pane.draft.as_ref().expect("a draft");
        assert_eq!(draft.kind(), Kind::Noul);
        assert_eq!(draft.fields().len(), 3);
        pane.next_field(2);
        type_in(&mut pane, "the reply is polite");
        assert!(matches!(enter(&mut pane), Judged::Asked(..)));
    }

    #[test]
    fn backspace_on_an_empty_option_takes_the_last_one_back() {
        let mut pane = judge_pane();
        pane.next_field(2);
        type_in(&mut pane, "keep: hold it");
        enter(&mut pane);
        pane.edit(Key::Backspace);
        let draft = pane.draft.as_ref().expect("a draft");
        assert!(draft.options.is_empty());
        assert_eq!(draft.option.as_str(), "keep: hold it");
    }

    #[test]
    fn changing_the_kind_reads_the_options_again() {
        let mut pane = judge_pane();
        pane.next_field(2);
        type_in(&mut pane, "refund: money back");
        enter(&mut pane);
        let draft = pane.draft.as_mut().expect("a draft");
        assert_eq!(
            draft.entries()[0],
            ("refund".to_owned(), "money back".to_owned())
        );
        draft.cycle_kind(1);
        assert_eq!(
            draft.entries()[0],
            ("refund: money back".to_owned(), String::new())
        );
        draft.cycle_kind(-1);
        assert_eq!(draft.entries()[0].1, "money back");
    }
}
