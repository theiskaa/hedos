//! Streaming text processors for model output: separating thinking spans from
//! visible text, and detecting stop sequences. Both hold back a partial-tag
//! suffix so a delimiter split across chunk boundaries is still recognized.

pub mod chunk;
pub mod stop_matcher;
pub mod think_splitter;
pub mod tools;

pub use chunk::{AudioFrame, CapabilityChunk, GenerationStats};
pub use stop_matcher::{StopMatcher, stop_strings};
pub use think_splitter::{Piece, TagPair, ThinkSplitter, has_visible_tags};
pub use tools::{ToolCall, ToolSpec};

/// The byte index at which the last `chars_from_end` characters of `text` begin.
/// Defined for `chars_from_end >= 1`; returns 0 when `text` has fewer characters
/// than requested.
pub(crate) fn char_boundary_from_end(text: &str, chars_from_end: usize) -> usize {
    if chars_from_end == 0 {
        return text.len();
    }
    text.char_indices()
        .rev()
        .nth(chars_from_end - 1)
        .map_or(0, |(index, _)| index)
}

/// The byte length of the longest suffix of `text` (up to one character shy of
/// the longest candidate) that is a prefix of some candidate delimiter. This is
/// the tail held back until the next chunk can confirm or deny a split delimiter.
pub(crate) fn held_suffix_len(text: &str, candidates: &[String]) -> usize {
    let max_chars = candidates
        .iter()
        .map(|candidate| candidate.chars().count())
        .max()
        .unwrap_or(1);
    let cap = max_chars.saturating_sub(1).min(text.chars().count());
    for length in (1..=cap).rev() {
        let start = char_boundary_from_end(text, length);
        let suffix = &text[start..];
        if candidates
            .iter()
            .any(|candidate| candidate.starts_with(suffix))
        {
            return text.len() - start;
        }
    }
    0
}
