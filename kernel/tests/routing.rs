//! The auto-routing policy: hard requirements rule a tier out, the latest user
//! turn picks among the rest, and a conversation that went large stays large.

use std::collections::BTreeMap;

use kernel::capabilities::ToolSpec;
use kernel::capabilities::chat::{AttachmentKind, ChatAttachment, ChatMessage, ChatRole};
use kernel::profiles::Verdict;
use kernel::records::{Capability, JsonValue, Modality, ModelRecord, ModelSource, SourceKind};
use kernel::routing::{
    ByTier, Candidate, Lack, RouteDecision, RouteInput, RouteReason, RouteRules, Signal, Tier,
    Tiers, conversation_key, decide, tool_chars,
};

fn record(name: &str, capabilities: &[Capability]) -> ModelRecord {
    ModelRecord::new(
        name,
        Modality::text(),
        capabilities.to_vec(),
        ModelSource::new(SourceKind::ollama(), name),
    )
}

fn chat_tools() -> Vec<Capability> {
    vec![Capability::chat(), Capability::tools()]
}

fn user(text: &str) -> ChatMessage {
    ChatMessage::new(ChatRole::User, text)
}

fn system(text: &str) -> ChatMessage {
    ChatMessage::new(ChatRole::System, text)
}

fn chat(messages: &[ChatMessage]) -> RouteInput {
    RouteInput::chat(messages, &[])
}

fn fits(record: &ModelRecord) -> Candidate<'_> {
    Candidate {
        record,
        fit: Verdict::Fits {
            clamped_max_tokens: None,
        },
        refused_parameter: None,
    }
}

fn exceeds(record: &ModelRecord) -> Candidate<'_> {
    Candidate {
        fit: Verdict::Exceeds {
            estimated: 10_000,
            window: 8192,
        },
        ..fits(record)
    }
}

fn tool() -> ToolSpec {
    ToolSpec::new("read", "Read a file", JsonValue::Object(BTreeMap::new()))
}

fn pair<'a>(small: &'a ModelRecord, large: &'a ModelRecord) -> Tiers<'a> {
    ByTier {
        small: Some(fits(small)),
        large: Some(fits(large)),
    }
}

fn route(input: &RouteInput, tiers: &Tiers<'_>, floor: Option<Tier>) -> RouteDecision {
    decide(input, tiers, floor, &RouteRules::default()).expect("a tier serves it")
}

#[test]
fn a_short_plain_turn_goes_small() {
    let (small, large) = (record("s", &chat_tools()), record("l", &chat_tools()));
    let decision = route(
        &chat(&[user("what does ls -la do?")]),
        &pair(&small, &large),
        None,
    );
    assert_eq!(decision.tier, Tier::Small);
    assert_eq!(decision.reason, RouteReason::Signal(Signal::ShortTurn));
}

#[test]
fn each_demanding_shape_goes_large_with_its_reason() {
    let (small, large) = (record("s", &chat_tools()), record("l", &chat_tools()));
    let long = "word ".repeat(400);
    let cases = [
        ("why?\n```rust\nfn main() {}\n```", Signal::CodeBlock),
        (
            "apply this\ndiff --git a/x b/x\n@@ -1 +1 @@\n-a\n+b",
            Signal::Diff,
        ),
        ("--- a/x.rs\n+++ b/x.rs\nchanged", Signal::Diff),
        (
            "it crashes:\nthread 'main' panicked at src/main.rs:3:5",
            Signal::StackTrace,
        ),
        (
            "Traceback (most recent call last):\n  File \"x.py\"",
            Signal::StackTrace,
        ),
        (
            "TypeError: x\n    at foo (a.js:1:2)\n    at bar (b.js:3:4)",
            Signal::StackTrace,
        ),
        (long.as_str(), Signal::LongTurn),
        (
            "please refactor the auth module",
            Signal::HardWord("refactor".to_owned()),
        ),
        (
            "walk me through it Step By Step",
            Signal::HardWord("step by step".to_owned()),
        ),
    ];
    for (text, signal) in cases {
        let decision = route(&chat(&[user(text)]), &pair(&small, &large), None);
        assert_eq!(decision.tier, Tier::Large, "{text:?}");
        assert_eq!(decision.reason, RouteReason::Signal(signal), "{text:?}");
    }
}

#[test]
fn hard_words_match_whole_words_only() {
    let (small, large) = (record("s", &chat_tools()), record("l", &chat_tools()));
    for text in ["the debugger window", "a redesigned page", "proven fact"] {
        let decision = route(&chat(&[user(text)]), &pair(&small, &large), None);
        assert_eq!(decision.tier, Tier::Small, "{text:?}");
    }
    let decision = route(
        &chat(&[user("Debug this, please")]),
        &pair(&small, &large),
        None,
    );
    assert_eq!(
        decision.reason,
        RouteReason::Signal(Signal::HardWord("debug".to_owned()))
    );
}

#[test]
fn a_single_at_line_is_prose_not_a_trace() {
    let (small, large) = (record("s", &chat_tools()), record("l", &chat_tools()));
    let decision = route(
        &chat(&[user("meet me\n  at noon: by the door")]),
        &pair(&small, &large),
        None,
    );
    assert_eq!(decision.tier, Tier::Small);
}

#[test]
fn the_system_prompt_and_old_turns_never_make_a_turn_long() {
    let (small, large) = (record("s", &chat_tools()), record("l", &chat_tools()));
    let messages = [
        system(&"rules ".repeat(4000)),
        user(&"old context ".repeat(500)),
        ChatMessage::new(ChatRole::Assistant, "done"),
        user("thanks, and what time is it in Tokyo?"),
    ];
    let decision = route(&chat(&messages), &pair(&small, &large), None);
    assert_eq!(decision.tier, Tier::Small);
}

#[test]
fn reminder_blocks_are_not_part_of_the_turn() {
    let (small, large) = (record("s", &chat_tools()), record("l", &chat_tools()));
    let injected = format!(
        "<system-reminder>{}</system-reminder>rename foo to bar",
        "context ".repeat(1000)
    );
    let decision = route(&chat(&[user(&injected)]), &pair(&small, &large), None);
    assert_eq!(decision.reason, RouteReason::Signal(Signal::ShortTurn));
}

#[test]
fn the_turn_is_the_last_user_message_with_text_of_its_own() {
    // Claude Code's tool results arrive as tool turns beside a user turn that
    // holds only a reminder; the person's request is the turn before.
    let (small, large) = (record("s", &chat_tools()), record("l", &chat_tools()));
    let mut result = ChatMessage::new(ChatRole::Tool, "file contents ".repeat(500));
    result.tool_call_id = Some("call_1".to_owned());
    let messages = [
        user("refactor the parser"),
        ChatMessage::new(ChatRole::Assistant, ""),
        user("<system-reminder>todo list is empty</system-reminder>"),
        result,
    ];
    let decision = route(&chat(&messages), &pair(&small, &large), None);
    assert_eq!(
        decision.reason,
        RouteReason::Signal(Signal::HardWord("refactor".to_owned()))
    );
}

#[test]
fn an_attached_document_goes_large() {
    let (small, large) = (record("s", &chat_tools()), record("l", &chat_tools()));
    let mut turn = user("summarize");
    turn.attachments.push(ChatAttachment {
        kind: AttachmentKind::Document,
        data: b"text".to_vec(),
        mime_type: "text/plain".to_owned(),
        name: Some("notes.txt".to_owned()),
    });
    let decision = route(&chat(&[turn]), &pair(&small, &large), None);
    assert_eq!(decision.reason, RouteReason::Signal(Signal::Document));
}

#[test]
fn tools_go_large_only_when_the_small_model_cannot_call_them() {
    let messages = [user("list the files")];
    let input = RouteInput::chat(&messages, &[tool()]);

    let (small, large) = (record("s", &chat_tools()), record("l", &chat_tools()));
    assert_eq!(route(&input, &pair(&small, &large), None).tier, Tier::Small);

    let small = record("s", &[Capability::chat()]);
    let decision = route(&input, &pair(&small, &large), None);
    assert_eq!(decision.tier, Tier::Large);
    assert_eq!(decision.reason.to_string(), "needs tools");
}

#[test]
fn images_need_a_model_that_sees() {
    let mut turn = user("what is in this picture?");
    turn.attachments.push(ChatAttachment {
        kind: AttachmentKind::Image,
        data: vec![0],
        mime_type: "image/png".to_owned(),
        name: None,
    });
    let input = chat(&[turn]);

    let small = record("s", &chat_tools());
    let large = record("l", &[Capability::chat(), Capability::see()]);
    let decision = route(&input, &pair(&small, &large), None);
    assert_eq!(decision.tier, Tier::Large);
    assert_eq!(decision.reason.to_string(), "needs vision");

    let large = record("l", &chat_tools());
    let refusal = decide(&input, &pair(&small, &large), None, &RouteRules::default())
        .expect_err("neither sees");
    assert_eq!(refusal.lacks.small, Lack::Capability(Capability::see()));
    assert_eq!(refusal.lacks.large, Lack::Capability(Capability::see()));
    assert_eq!(
        refusal.to_string(),
        "neither routed model can serve this request: small lacks vision; large lacks vision"
    );
}

#[test]
fn only_an_image_in_the_last_message_needs_vision() {
    // The runtime only checks the last message for images, so a picture
    // earlier in the conversation must not pin it to the model that sees.
    let mut picture = user("what is in this picture?");
    picture.attachments.push(ChatAttachment {
        kind: AttachmentKind::Image,
        data: vec![0],
        mime_type: "image/png".to_owned(),
        name: None,
    });
    let messages = [
        picture,
        ChatMessage::new(ChatRole::Assistant, "a cat"),
        user("thanks"),
    ];
    let small = record("s", &chat_tools());
    let large = record("l", &[Capability::chat(), Capability::see()]);
    assert_eq!(
        route(&chat(&messages), &pair(&small, &large), None).tier,
        Tier::Small
    );
}

#[test]
fn a_parameter_the_small_runtime_refuses_goes_large() {
    let (small, large) = (record("s", &chat_tools()), record("l", &chat_tools()));
    let tiers = ByTier {
        small: Some(Candidate {
            refused_parameter: Some("presence_penalty".to_owned()),
            ..fits(&small)
        }),
        large: Some(fits(&large)),
    };
    let decision = route(&chat(&[user("hi")]), &tiers, None);
    assert_eq!(decision.tier, Tier::Large);
    assert_eq!(decision.reason.to_string(), "needs presence_penalty");
}

#[test]
fn a_prompt_past_the_small_window_goes_large() {
    let (small, large) = (record("s", &chat_tools()), record("l", &chat_tools()));
    let messages = [user("and now?")];
    let tiers = ByTier {
        small: Some(exceeds(&small)),
        large: Some(fits(&large)),
    };
    let decision = route(&chat(&messages), &tiers, None);
    assert_eq!(decision.tier, Tier::Large);
    assert_eq!(decision.reason.to_string(), "needs a wider window");
}

#[test]
fn tool_definitions_are_measured() {
    let big = ToolSpec::new("big", "x".repeat(40_000), JsonValue::Null);
    assert_eq!(tool_chars(&[big]), 3 + 40_000 + "null".len() as i64);
    assert_eq!(tool_chars(&[]), 0);
}

#[test]
fn a_conversation_that_went_large_stays_large() {
    let (small, large) = (record("s", &chat_tools()), record("l", &chat_tools()));
    let decision = route(
        &chat(&[user("ok")]),
        &pair(&small, &large),
        Some(Tier::Large),
    );
    assert_eq!(decision.tier, Tier::Large);
    assert_eq!(decision.reason, RouteReason::Sticky);

    let decision = route(
        &chat(&[user("ok")]),
        &pair(&small, &large),
        Some(Tier::Small),
    );
    assert_eq!(decision.tier, Tier::Small);
}

#[test]
fn a_missing_tier_hands_its_requests_to_the_other() {
    let large = record("l", &chat_tools());
    let tiers = ByTier {
        small: None,
        large: Some(fits(&large)),
    };
    let decision = route(&chat(&[user("hi")]), &tiers, None);
    assert_eq!(decision.tier, Tier::Large);
    assert_eq!(decision.reason.to_string(), "small not ready");

    let small = record("s", &chat_tools());
    let tiers = ByTier {
        small: Some(fits(&small)),
        large: None,
    };
    let decision = route(&chat(&[user("refactor it")]), &tiers, Some(Tier::Large));
    assert_eq!(decision.tier, Tier::Small);
    assert_eq!(decision.reason.to_string(), "large is not ready");
}

#[test]
fn no_tier_at_all_is_a_refusal_that_says_so() {
    let refusal = decide(
        &chat(&[user("hi")]),
        &Tiers::default(),
        None,
        &RouteRules::default(),
    )
    .expect_err("nothing to route to");
    assert!(refusal.nothing_ready());
}

#[test]
fn a_completion_needs_the_complete_capability() {
    let small = record("s", &chat_tools());
    let large = record("l", &[Capability::chat(), Capability::complete()]);
    let input = RouteInput::prompt("Once upon a time");
    let decision = route(&input, &pair(&small, &large), None);
    assert_eq!(decision.tier, Tier::Large);
    assert_eq!(decision.reason.to_string(), "needs complete");
}

#[test]
fn custom_rules_replace_the_defaults() {
    let (small, large) = (record("s", &chat_tools()), record("l", &chat_tools()));
    let rules = RouteRules {
        max_simple_chars: 10,
        hard_words: vec!["fix".to_owned()],
    };
    let tiers = pair(&small, &large);
    let long = decide(&chat(&[user("a short-ish turn")]), &tiers, None, &rules).unwrap();
    assert_eq!(long.reason, RouteReason::Signal(Signal::LongTurn));
    let word = decide(&chat(&[user("fix it")]), &tiers, None, &rules).unwrap();
    assert_eq!(word.tier, Tier::Large);
    let default_word = decide(&chat(&[user("refactor")]), &tiers, None, &rules).unwrap();
    assert_eq!(default_word.tier, Tier::Small);
}

#[test]
fn the_conversation_key_holds_across_turns_and_differs_between_conversations() {
    let first = [system("be brief"), user("hello")];
    let later = [
        system("be brief"),
        user("hello"),
        ChatMessage::new(ChatRole::Assistant, "hi"),
        user("and again"),
    ];
    let other = [system("be brief"), user("goodbye")];
    let key = conversation_key(&first).expect("a user message anchors it");
    assert_eq!(conversation_key(&later), Some(key));
    assert_ne!(conversation_key(&other), Some(key));
    assert_ne!(
        conversation_key(&[system("be verbose"), user("hello")]),
        Some(key)
    );
    assert_eq!(conversation_key(&[system("only a system prompt")]), None);
}

#[test]
fn a_hard_word_with_punctuation_matches_as_written() {
    let (small, large) = (record("s", &chat_tools()), record("l", &chat_tools()));
    let rules = RouteRules {
        hard_words: vec!["C++".to_owned()],
        ..RouteRules::default()
    };
    let tiers = pair(&small, &large);
    let plain_c = decide(&chat(&[user("is c faster than go?")]), &tiers, None, &rules).unwrap();
    assert_eq!(plain_c.tier, Tier::Small, "a bare c is not c++");
    let cpp = decide(&chat(&[user("port this to c++")]), &tiers, None, &rules).unwrap();
    assert_eq!(
        cpp.reason,
        RouteReason::Signal(Signal::HardWord("c++".to_owned()))
    );
}
