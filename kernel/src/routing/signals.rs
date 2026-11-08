//! Reading how demanding a request is from the latest user turn alone.
//!
//! Agent harnesses send a long system prompt and their whole tool list with
//! every request, so the prompt as a whole says nothing about the task. The
//! turn the person last typed does.

use std::fmt;

use crate::capabilities::chat::{AttachmentKind, ChatMessage, ChatRole};

use super::{RouteRules, Tier};

/// Why a turn was judged simple or demanding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// Nothing in the turn asks for the large model.
    ShortTurn,
    /// The turn is longer than the simple-turn limit.
    LongTurn,
    /// The turn carries a fenced code block.
    CodeBlock,
    /// The turn carries a unified diff.
    Diff,
    /// The turn carries a stack trace or a panic.
    StackTrace,
    /// The turn carries an attached document.
    Document,
    /// The turn uses one of the hard words.
    HardWord(String),
}

impl Signal {
    /// The tier this signal asks for.
    pub fn tier(&self) -> Tier {
        match self {
            Signal::ShortTurn => Tier::Small,
            _ => Tier::Large,
        }
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Signal::ShortTurn => f.write_str("short turn"),
            Signal::LongTurn => f.write_str("long turn"),
            Signal::CodeBlock => f.write_str("code in turn"),
            Signal::Diff => f.write_str("diff in turn"),
            Signal::StackTrace => f.write_str("stack trace in turn"),
            Signal::Document => f.write_str("document attached"),
            Signal::HardWord(word) => write!(f, "hard word \"{word}\""),
        }
    }
}

/// The text of the latest user turn, and whether it carries a document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Turn {
    /// The turn's text, with harness-injected reminder blocks removed.
    pub text: String,
    /// Whether the turn carries an attached document.
    pub document: bool,
}

impl Turn {
    /// The latest user turn in `messages` that still has text once reminder
    /// blocks are removed. Claude Code sends tool results as user turns with
    /// only a reminder beside them, and those are not something the person
    /// typed. Empty when no user turn has text.
    pub fn latest(messages: &[ChatMessage]) -> Self {
        for message in messages.iter().rev() {
            if message.role != ChatRole::User {
                continue;
            }
            let text = without_reminders(&message.content);
            let document = message
                .attachments
                .iter()
                .any(|attachment| attachment.kind == AttachmentKind::Document);
            if !text.trim().is_empty() || document {
                return Self {
                    text: text.trim().to_owned(),
                    document,
                };
            }
        }
        Self::default()
    }

    /// A bare prompt, as the completion endpoints carry one.
    pub fn prompt(prompt: &str) -> Self {
        Self {
            text: without_reminders(prompt).trim().to_owned(),
            document: false,
        }
    }
}

/// Judge `turn` against `rules`. The first signal that fires is the one
/// reported; a turn none fire on is a short turn.
pub fn classify(turn: &Turn, rules: &RouteRules) -> Signal {
    if turn.document {
        return Signal::Document;
    }
    let text = turn.text.as_str();
    if text.contains("```") {
        return Signal::CodeBlock;
    }
    if has_diff(text) {
        return Signal::Diff;
    }
    if has_stack_trace(text) {
        return Signal::StackTrace;
    }
    if text.chars().count() > rules.max_simple_chars {
        return Signal::LongTurn;
    }
    if let Some(word) = first_hard_word(text, &rules.hard_words) {
        return Signal::HardWord(word);
    }
    Signal::ShortTurn
}

/// The opening tag Claude Code wraps its injected context in.
const REMINDER_OPEN: &str = "<system-reminder>";
const REMINDER_CLOSE: &str = "</system-reminder>";

/// `text` with every `<system-reminder>` block cut out. An unclosed block runs
/// to the end of the text.
fn without_reminders(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(REMINDER_OPEN) {
        kept.push_str(&rest[..start]);
        let after = &rest[start + REMINDER_OPEN.len()..];
        match after.find(REMINDER_CLOSE) {
            Some(end) => rest = &after[end + REMINDER_CLOSE.len()..],
            None => return kept,
        }
    }
    kept.push_str(rest);
    kept
}

fn has_diff(text: &str) -> bool {
    let mut minus = false;
    let mut plus = false;
    for line in text.lines() {
        if line.starts_with("diff --git ") || line.starts_with("@@ -") {
            return true;
        }
        minus |= line.starts_with("--- a/");
        plus |= line.starts_with("+++ b/");
    }
    minus && plus
}

fn has_stack_trace(text: &str) -> bool {
    if text.contains("Traceback (most recent call last)")
        || text.contains("panicked at")
        || text.contains("Exception in thread")
        || text.contains("stack backtrace:")
    {
        return true;
    }
    // Two JavaScript or Java frames (`    at name (file:line)`) make a trace;
    // one could be prose.
    text.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            line.len() > trimmed.len() && trimmed.starts_with("at ") && trimmed.contains(':')
        })
        .take(2)
        .count()
        == 2
}

/// The first of `words` that appears in `text`, compared case-insensitively.
/// A word or phrase of letters and digits matches as whole consecutive words;
/// one carrying anything else (`c++`, `.unwrap()`) matches as written, since
/// splitting it into words would leave too little to match on.
fn first_hard_word(text: &str, words: &[String]) -> Option<String> {
    let lowered = text.to_lowercase();
    let haystack = split_words(text);
    words.iter().find_map(|word| {
        let word = word.trim().to_lowercase();
        let plain = word
            .chars()
            .all(|c| c.is_alphanumeric() || c.is_whitespace());
        let found = if plain {
            let needle = split_words(&word);
            !needle.is_empty()
                && needle.len() <= haystack.len()
                && haystack
                    .windows(needle.len())
                    .any(|window| window == needle.as_slice())
        } else {
            !word.is_empty() && lowered.contains(&word)
        };
        found.then_some(word)
    })
}

fn split_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}
