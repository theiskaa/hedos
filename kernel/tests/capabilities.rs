//! Integration tests for the `capabilities` streaming processors: the
//! ThinkSplitter (separating thinking spans) and StopMatcher (stop-sequence
//! detection), with emphasis on delimiters split across chunk boundaries.

mod support;

use kernel::capabilities::{
    Piece, StopMatcher, TagPair, ThinkSplitter, has_visible_tags, stop_strings,
};
use kernel::records::JsonValue;

fn run_think(splitter: &mut ThinkSplitter, chunks: &[&str]) -> (String, String) {
    let mut text = String::new();
    let mut thinking = String::new();
    let mut absorb = |pieces: Vec<Piece>| {
        for piece in pieces {
            match piece {
                Piece::Text(value) => text.push_str(&value),
                Piece::Thinking(value) => thinking.push_str(&value),
            }
        }
    };
    for chunk in chunks {
        absorb(splitter.feed(chunk));
    }
    absorb(splitter.flush());
    (text, thinking)
}

#[test]
fn think_splitter_passes_plain_text_through() {
    let (text, thinking) = run_think(&mut ThinkSplitter::new(), &["just some text"]);
    assert_eq!(text, "just some text");
    assert!(thinking.is_empty());
}

#[test]
fn think_splitter_separates_a_thinking_span() {
    let (text, thinking) = run_think(&mut ThinkSplitter::new(), &["a<think>b</think>c"]);
    assert_eq!(text, "ac");
    assert_eq!(thinking, "b");
}

#[test]
fn think_splitter_handles_tags_split_across_chunks() {
    let (text, thinking) = run_think(
        &mut ThinkSplitter::new(),
        &["a<thi", "nk>reason", "ing</thi", "nk>done"],
    );
    assert_eq!(text, "adone");
    assert_eq!(thinking, "reasoning");
}

#[test]
fn think_splitter_holds_back_a_partial_tag() {
    let mut splitter = ThinkSplitter::new();
    assert_eq!(
        splitter.feed("hello<"),
        vec![Piece::Text("hello".to_owned())],
        "the lone '<' is held back until the tag resolves"
    );
    let (text, thinking) = run_think(&mut splitter, &["think>x", "</think>!"]);
    assert_eq!(text, "!");
    assert_eq!(thinking, "x");
}

#[test]
fn think_splitter_supports_the_alternate_delimiter() {
    let (text, thinking) = run_think(
        &mut ThinkSplitter::new(),
        &["a<|START_THINKING|>b<|END_THINKING|>c"],
    );
    assert_eq!(text, "ac");
    assert_eq!(thinking, "b");
}

#[test]
fn think_splitter_flush_classifies_unterminated_thinking() {
    let (text, thinking) = run_think(&mut ThinkSplitter::new(), &["a<think>unterminated"]);
    assert_eq!(text, "a");
    assert_eq!(thinking, "unterminated");
}

#[test]
fn think_splitter_custom_pairs() {
    let pairs = vec![TagPair {
        open: "[[".to_owned(),
        close: "]]".to_owned(),
    }];
    let (text, thinking) = run_think(&mut ThinkSplitter::with_pairs(pairs), &["a[[b]]c"]);
    assert_eq!(text, "ac");
    assert_eq!(thinking, "b");
}

#[test]
fn has_visible_tags_detects_default_delimiters() {
    assert!(has_visible_tags("before <think> after"));
    assert!(has_visible_tags("x</think>"));
    assert!(has_visible_tags("<|START_THINKING|>"));
    assert!(!has_visible_tags("no tags here"));
}

fn run_stop(stops: &[&str], chunks: &[&str]) -> (String, bool) {
    let mut matcher = StopMatcher::new(stops.iter().map(|s| (*s).to_owned()).collect());
    let mut emitted = String::new();
    for chunk in chunks {
        emitted.push_str(&matcher.feed(chunk));
    }
    emitted.push_str(&matcher.flush());
    (emitted, matcher.is_stopped())
}

#[test]
fn stop_matcher_passes_through_when_inactive() {
    let (emitted, stopped) = run_stop(&[], &["anything at all"]);
    assert_eq!(emitted, "anything at all");
    assert!(!stopped);
}

#[test]
fn stop_matcher_cuts_at_the_stop_sequence() {
    let (emitted, stopped) = run_stop(&["STOP"], &["hello STOP world"]);
    assert_eq!(emitted, "hello ");
    assert!(stopped);
}

#[test]
fn stop_matcher_handles_a_stop_split_across_chunks() {
    let (emitted, stopped) = run_stop(&["</s>"], &["abc</", "s>trailing"]);
    assert_eq!(emitted, "abc");
    assert!(stopped);
}

#[test]
fn stop_matcher_holds_back_a_partial_then_releases_it() {
    let mut matcher = StopMatcher::new(vec!["</s>".to_owned()]);
    assert_eq!(matcher.feed("done</"), "done", "the partial '</' is held");
    assert_eq!(
        matcher.feed("x more"),
        "</x more",
        "a non-match releases the held text"
    );
    assert!(!matcher.is_stopped());
}

#[test]
fn stop_matcher_emits_remaining_on_flush() {
    let (emitted, stopped) = run_stop(&["NEVER"], &["some text"]);
    assert_eq!(emitted, "some text");
    assert!(!stopped);
}

#[test]
fn stop_matcher_uses_the_earliest_of_several_stops() {
    let (emitted, stopped) = run_stop(&["END", "STOP"], &["go STOP then END"]);
    assert_eq!(emitted, "go ");
    assert!(stopped);
}

#[test]
fn stop_matcher_ignores_empty_stops() {
    let matcher = StopMatcher::new(vec![String::new()]);
    assert!(!matcher.is_active());
}

#[test]
fn stop_strings_extracts_from_param_values() {
    assert_eq!(stop_strings(Some(&JsonValue::from("</s>"))), vec!["</s>"]);
    let array = JsonValue::Array(vec![
        JsonValue::from("a"),
        JsonValue::Int(1),
        JsonValue::from("b"),
    ]);
    assert_eq!(stop_strings(Some(&array)), vec!["a", "b"]);
    assert!(stop_strings(None).is_empty());
    assert!(stop_strings(Some(&JsonValue::Int(5))).is_empty());
}

#[test]
fn think_splitter_is_utf8_safe_with_ascii_tags() {
    let (text, thinking) = run_think(&mut ThinkSplitter::new(), &["café<think>déjà</think>naïve"]);
    assert_eq!(text, "cafénaïve");
    assert_eq!(thinking, "déjà");
}

#[test]
fn think_splitter_handles_multibyte_delimiters_across_chunks() {
    let pairs = vec![TagPair {
        open: "⟪".to_owned(),
        close: "⟫".to_owned(),
    }];
    let mut splitter = ThinkSplitter::with_pairs(pairs);
    let (text, thinking) = run_think(&mut splitter, &["a⟪b", "cd⟫e"]);
    assert_eq!(text, "ae");
    assert_eq!(thinking, "bcd");
}

#[test]
fn think_splitter_handles_empty_and_back_to_back_spans() {
    let (text, thinking) = run_think(&mut ThinkSplitter::new(), &["<think></think>x"]);
    assert_eq!(text, "x");
    assert_eq!(thinking, "");

    let (text, thinking) = run_think(
        &mut ThinkSplitter::new(),
        &["<think>a</think><think>b</think>"],
    );
    assert_eq!(text, "");
    assert_eq!(thinking, "ab");
}

#[test]
fn think_splitter_treats_a_stray_close_tag_as_text() {
    let (text, thinking) = run_think(&mut ThinkSplitter::new(), &["</think>hello"]);
    assert_eq!(text, "</think>hello");
    assert!(thinking.is_empty());
}

#[test]
fn think_splitter_empty_delimiter_pairs_are_dropped() {
    let pairs = vec![TagPair {
        open: String::new(),
        close: String::new(),
    }];
    let (text, thinking) = run_think(&mut ThinkSplitter::with_pairs(pairs), &["plain text"]);
    assert_eq!(text, "plain text");
    assert!(thinking.is_empty());
}

#[test]
fn think_splitter_handles_empty_feeds() {
    let mut splitter = ThinkSplitter::new();
    assert!(splitter.feed("").is_empty());
    let (text, _) = run_think(&mut splitter, &["", "hi", ""]);
    assert_eq!(text, "hi");
}

#[test]
fn stop_matcher_holds_multibyte_stop_across_chunks() {
    let (emitted, stopped) = run_stop(&["→END"], &["a→", "ENDx"]);
    assert_eq!(emitted, "a");
    assert!(stopped);
}

#[test]
fn stop_matcher_accumulates_across_many_single_char_feeds() {
    let (emitted, stopped) = run_stop(&["ABCDE"], &["A", "B", "C", "D", "E", "F"]);
    assert_eq!(emitted, "");
    assert!(stopped);
}

#[test]
fn stop_matcher_flush_releases_a_held_partial() {
    let mut matcher = StopMatcher::new(vec!["</s>".to_owned()]);
    assert_eq!(matcher.feed("text</"), "text");
    assert_eq!(
        matcher.flush(),
        "</",
        "the held partial is released on flush"
    );
}
