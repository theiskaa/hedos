//! The decision wire of a llama.cpp server: a typed question put to a decision
//! model (clef, laya, kev, openjev, lev) over llama-server's `/v1/systemone`,
//! and what its refusals mean.
//!
//! The question travels as the exact text the caller wrote, never re-encoded:
//! the order of a choice question's options changes the probabilities the
//! model gives them, and every JSON object this crate parses comes out sorted
//! by key. The envelope comes back the same way.

use kernel::capabilities::question_carries_images;
use kernel::records::JsonValue;

use super::RuntimeError;
use super::openai::{ErrorBody, PostFailure, post_capped, tokens_before};

/// What a decision server answered: the System One envelope as it sent it,
/// and the prompt tokens it read when it said.
#[derive(Debug)]
pub(crate) struct Answer {
    /// The envelope, byte for byte.
    pub(crate) envelope: String,
    /// `usage.input_tokens`, when the envelope carries it.
    pub(crate) input_tokens: Option<i64>,
}

/// The question a judge payload carries: the text of its last user message,
/// which both the gateway and `hedos run` fill with
/// `{"state": …, "questions": …}`.
pub(crate) fn question_text(payload: &JsonValue) -> Result<String, RuntimeError> {
    payload
        .as_object()
        .and_then(|fields| fields.get("messages"))
        .and_then(JsonValue::as_array)
        .and_then(|messages| {
            messages.iter().rev().find_map(|message| {
                let fields = message.as_object()?;
                (fields.get("role")?.as_str()? == "user")
                    .then(|| fields.get("content")?.as_str())
                    .flatten()
            })
        })
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            RuntimeError::Rejected(
                "a typed question is a user message holding {\"state\": …, \"questions\": …}"
                    .to_owned(),
            )
        })
}

/// Post `question` to `{base}/v1/systemone` and read back the envelope.
/// `label` names the server in an error, which never carries its address.
pub(crate) async fn ask(
    client: &reqwest::Client,
    base: &str,
    question: &str,
    label: &str,
) -> Result<Answer, PostFailure> {
    let url = format!("{base}/v1/systemone");
    let body = question.as_bytes().to_vec();
    let bytes = post_capped(client, &url, body, super::MAX_JSON_BODY_BYTES, label).await?;
    let envelope = String::from_utf8(bytes).map_err(|_| {
        PostFailure::Runtime(RuntimeError::Failed(format!(
            "{label} answered with a body that is not text"
        )))
    })?;
    let input_tokens = serde_json::from_str::<JsonValue>(&envelope)
        .ok()
        .as_ref()
        .and_then(JsonValue::as_object)
        .and_then(|fields| fields.get("usage"))
        .and_then(JsonValue::as_object)
        .and_then(|usage| usage.get("input_tokens"))
        .and_then(JsonValue::as_i64);
    Ok(Answer {
        envelope,
        input_tokens,
    })
}

/// What it takes to serve a decision model, said when the server on `PATH`
/// cannot.
pub(crate) fn upgrade_hint(name: &str) -> String {
    format!(
        "{name} is a decision model, which needs llama.cpp 0.6.0 or newer (brew upgrade llama.cpp)"
    )
}

/// The ways llama-server says a prompt is longer than it reads at once: the
/// text before its token count, the verdict after it, and what is too long,
/// as this module says it. The first is sent as a server error, though it is
/// the caller's prompt that is too long.
const TOO_LONG: [(&str, &str, &str); 4] = [
    ("input (", "is too large to process", "the question takes"),
    (
        "input (",
        "is larger than the max context size",
        "the question takes",
    ),
    (
        "request (",
        "exceeds the available context size",
        "the question takes",
    ),
    (
        "the question and its options (",
        "are too large to process",
        "the question and its options take",
    ),
];

/// The error a refused decision request is for the caller: `status` and
/// `body` as llama-server answered `question`, put to `name` on a server of
/// `window` tokens.
pub(crate) fn refusal(
    status: u16,
    body: ErrorBody,
    question: &str,
    window: i64,
    name: &str,
) -> RuntimeError {
    let message = body.message.unwrap_or_default();
    for (lead, verdict, subject) in TOO_LONG {
        if let Some(tokens) = tokens_before(&message, lead, verdict) {
            return RuntimeError::Rejected(match tokens {
                Some(tokens) => format!(
                    "{subject} {tokens} tokens, more than the {window}-token window {name} is served with"
                ),
                None => {
                    format!("{subject} more than the {window}-token window {name} is served with")
                }
            });
        }
    }
    let said = |fallback: String| {
        if message.is_empty() {
            fallback
        } else {
            message.clone()
        }
    };
    match status {
        400 => RuntimeError::Rejected(said(format!("{name} refused the question"))),
        404 => RuntimeError::Unavailable(format!(
            "{}; this llama-server has no /v1/systemone",
            upgrade_hint(name)
        )),
        // llama-server's "not supported" for images is the caller's to fix,
        // and its advice names a flag the caller cannot set, so it is not
        // passed on. Any other 501 means llama.cpp does not see a decision
        // model where the kernel did: the server's failure, not the caller's.
        501 if message.contains("does not support image input")
            && question_carries_images(question) =>
        {
            RuntimeError::Rejected(format!("{name} does not read images"))
        }
        _ => RuntimeError::Failed(said(format!("llama-server answered with HTTP {status}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(message: &str) -> ErrorBody {
        ErrorBody {
            message: Some(message.to_owned()),
            kind: None,
        }
    }

    fn rejected(error: RuntimeError) -> String {
        match error {
            RuntimeError::Rejected(message) => message,
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    const QUESTION: &str = r#"{"state":"s","questions":{"q":{"type":"noul","instructions":"i"}}}"#;

    const NO_IMAGES: &str = "This server does not support image input for decisions. For a model that supports it, start it with `--mmproj`";

    #[test]
    fn the_question_is_the_last_user_messages_text() {
        let payload = JsonValue::object([(
            "messages",
            JsonValue::Array(vec![
                JsonValue::object([
                    ("role", JsonValue::String("user".to_owned())),
                    ("content", JsonValue::String("first".to_owned())),
                ]),
                JsonValue::object([
                    ("role", JsonValue::String("user".to_owned())),
                    ("content", JsonValue::String(QUESTION.to_owned())),
                ]),
                JsonValue::object([
                    ("role", JsonValue::String("assistant".to_owned())),
                    ("content", JsonValue::String("no".to_owned())),
                ]),
            ]),
        )]);
        assert_eq!(question_text(&payload).unwrap(), QUESTION);
    }

    #[test]
    fn a_payload_with_no_question_is_the_callers_fault() {
        let empty = JsonValue::object([(
            "messages",
            JsonValue::Array(vec![JsonValue::object([
                ("role", JsonValue::String("user".to_owned())),
                ("content", JsonValue::String("  ".to_owned())),
            ])]),
        )]);
        for payload in [
            JsonValue::object([("prompt", JsonValue::String("x".to_owned()))]),
            empty,
        ] {
            assert!(matches!(
                question_text(&payload),
                Err(RuntimeError::Rejected(_))
            ));
        }
    }

    #[test]
    fn every_too_long_refusal_is_the_callers_and_names_the_window() {
        for (status, message, subject) in [
            (
                500,
                "input (20000 tokens) is too large to process. increase the physical batch size (current batch size: 16384)",
                "the question takes 20000 tokens",
            ),
            (
                400,
                "input (20000 tokens) is larger than the max context size (16384 tokens). skipping",
                "the question takes 20000 tokens",
            ),
            (
                400,
                "request (20000 tokens) exceeds the available context size (16384 tokens), try increasing it",
                "the question takes 20000 tokens",
            ),
            (
                400,
                "the question and its options (20000 tokens) are too large to process. increase the batch size (current batch size: 16384)",
                "the question and its options take 20000 tokens",
            ),
        ] {
            let message = rejected(refusal(status, body(message), QUESTION, 16384, "clef"));
            assert_eq!(
                message,
                format!("{subject}, more than the 16384-token window clef is served with")
            );
        }
    }

    #[test]
    fn a_bad_request_is_the_callers_with_the_servers_reason() {
        let message = rejected(refusal(
            400,
            body("choice question has too many options"),
            QUESTION,
            16384,
            "openjev",
        ));
        assert_eq!(message, "choice question has too many options");
    }

    #[test]
    fn a_server_without_systemone_names_the_upgrade() {
        match refusal(404, ErrorBody::default(), QUESTION, 16384, "clef") {
            RuntimeError::Unavailable(message) => {
                assert!(message.contains("llama.cpp 0.6.0 or newer"), "{message}");
                assert!(message.contains("brew upgrade llama.cpp"), "{message}");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn not_supported_is_the_callers_only_when_the_question_holds_images() {
        let with_images = r#"{"state":"s","questions":{},"images":["data:image/png;base64,AA"]}"#;
        let in_state = r#"{"state":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA"}}]}],"questions":{}}"#;
        for question in [with_images, in_state] {
            assert_eq!(
                rejected(refusal(501, body(NO_IMAGES), question, 16384, "kev")),
                "kev does not read images"
            );
        }
        assert!(matches!(
            refusal(501, body("This model is not a decision model"), QUESTION, 16384, "kev"),
            RuntimeError::Failed(message) if message == "This model is not a decision model"
        ));
        // Not a decision model is the server's failure, images or not.
        assert!(matches!(
            refusal(
                501,
                body("This model is not a decision model"),
                with_images,
                16384,
                "kev"
            ),
            RuntimeError::Failed(_)
        ));
        assert!(matches!(
            refusal(500, ErrorBody::default(), QUESTION, 16384, "kev"),
            RuntimeError::Failed(message) if message == "llama-server answered with HTTP 500"
        ));
    }
}
