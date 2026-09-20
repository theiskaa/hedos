//! TypeSafe's System One wire: a state, a set of typed questions (choice, score,
//! noul) and a model name in, an envelope of typed answers out.
//!
//! Nothing here models the questions or the answers. The model that answers
//! them already speaks this shape on both sides, so the request's `questions`
//! and `state` are carried to it as the exact text the client wrote, and its
//! reply goes back as the exact text it produced. That is for correctness more
//! than economy: the order of a choice question's options changes the
//! probabilities the model gives them, a client that cares writes its body by
//! hand to keep that order, and every JSON object this crate parses comes out
//! sorted by key. The parsed copy below exists only to be checked.

use std::collections::BTreeMap;

use kernel::records::JsonValue;
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;

use crate::error::GatewayError;
use crate::handlers::{bad_request, server_error};

/// The question types System One defines.
const QUESTION_TYPES: [&str; 3] = ["choice", "score", "noul"];

/// A decoded System One request, borrowing the body it came from.
#[derive(Debug)]
pub struct SystemOneRequest<'a> {
    /// The model the client named.
    pub model: String,
    questions: &'a RawValue,
    state: &'a RawValue,
}

#[derive(Deserialize)]
struct RawRequest<'a> {
    model: Option<String>,
    #[serde(borrow, default, deserialize_with = "present")]
    questions: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "present")]
    state: Option<&'a RawValue>,
}

/// Deserialize a field that is there, `null` included. A plain `Option` reads
/// `null` as absent, and `null` is a state a client may mean.
fn present<'de: 'a, 'a, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<&'a RawValue>, D::Error> {
    <&RawValue>::deserialize(deserializer).map(Some)
}

/// Decode and check a System One request body. Every fault a caller can commit
/// is refused here as a `400`, before a model is involved: the TypeSafe SDK
/// retries a `5xx`, so a malformed request answered by the runtime would be
/// sent, and fail, three times.
pub fn decode_request(body: &[u8]) -> Result<SystemOneRequest<'_>, GatewayError> {
    let raw: RawRequest<'_> = serde_json::from_slice(body)
        .map_err(|_| bad_request("request body must be a JSON object"))?;
    let model = raw
        .model
        .filter(|model| !model.is_empty())
        .ok_or_else(|| bad_request("model is required"))?;
    let questions = raw
        .questions
        .ok_or_else(|| bad_request("questions is required"))?;
    let state = raw.state.ok_or_else(|| bad_request("state is required"))?;
    check_questions(questions)?;
    Ok(SystemOneRequest {
        model,
        questions,
        state,
    })
}

fn check_questions(questions: &RawValue) -> Result<(), GatewayError> {
    let parsed: JsonValue = serde_json::from_str(questions.get())
        .map_err(|_| bad_request("questions must be a JSON object"))?;
    let Some(questions) = parsed.as_object().filter(|fields| !fields.is_empty()) else {
        return Err(bad_request(
            "questions must be an object holding at least one question",
        ));
    };
    for (id, question) in questions {
        let Some(fields) = question.as_object() else {
            return Err(bad_request(format!("question \"{id}\" must be an object")));
        };
        let known_type = fields
            .get("type")
            .and_then(JsonValue::as_str)
            .is_some_and(|kind| QUESTION_TYPES.contains(&kind));
        if !known_type {
            return Err(bad_request(format!(
                "question \"{id}\" needs a type of choice, score, or noul"
            )));
        }
        if !fields.contains_key("instructions") {
            return Err(bad_request(format!("question \"{id}\" needs instructions")));
        }
        check_criteria(id, fields)?;
    }
    Ok(())
}

/// A choice needs options to choose between (an object of label to description,
/// or a list of labels) and a score needs its list of levels. A noul's criteria
/// are optional. Without this the model fails on the missing field with an
/// error about its own internals.
fn check_criteria(id: &str, fields: &BTreeMap<String, JsonValue>) -> Result<(), GatewayError> {
    let criteria = fields.get("criteria");
    let listed = criteria
        .and_then(JsonValue::as_array)
        .is_some_and(|items| !items.is_empty());
    let labelled = criteria
        .and_then(JsonValue::as_object)
        .is_some_and(|items| !items.is_empty());
    match fields.get("type").and_then(JsonValue::as_str) {
        Some("choice") if !listed && !labelled => Err(bad_request(format!(
            "question \"{id}\" is a choice and needs criteria to choose between"
        ))),
        Some("score") if !listed => Err(bad_request(format!(
            "question \"{id}\" is a score and needs a list of levels as criteria"
        ))),
        _ => Ok(()),
    }
}

impl SystemOneRequest<'_> {
    /// The question as the model reads it: `{"state": …, "questions": …}`, with
    /// both values spliced in as the client's own text so that no key moves.
    pub fn question_text(&self) -> String {
        format!(
            "{{\"state\":{},\"questions\":{}}}",
            self.state.get(),
            self.questions.get()
        )
    }

    /// The chat payload that carries [`question_text`](Self::question_text) to
    /// the model as a single user message.
    pub fn chat_payload(&self) -> JsonValue {
        JsonValue::object([(
            "messages",
            JsonValue::Array(vec![JsonValue::object([
                ("role", JsonValue::String("user".to_owned())),
                ("content", JsonValue::String(self.question_text())),
            ])]),
        )])
    }
}

/// The response body for a model's `reply`: the reply itself, byte for byte,
/// once it is seen to be an envelope with answers in it. A reply that is
/// anything else means the model behind the route does not speak System One,
/// which is this machine's fault and not the caller's.
pub fn envelope(reply: &str) -> Result<Vec<u8>, GatewayError> {
    let parsed: JsonValue = serde_json::from_str(reply)
        .map_err(|_| server_error("the model's reply was not a System One envelope"))?;
    let has_answers = parsed
        .as_object()
        .and_then(|fields| fields.get("answers"))
        .is_some_and(|answers| answers.as_object().is_some());
    if !has_answers {
        return Err(server_error(
            "the model's reply was not a System One envelope",
        ));
    }
    Ok(reply.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::GatewayErrorKind;

    const CHOICE: &str = r#"{"model":"laya","questions":{"route":{"type":"choice","instructions":"Which team?","criteria":{"zeta":"last letter","alpha":"first letter","mid":"between"}}},"state":{"z":1,"a":2}}"#;

    #[test]
    fn the_question_text_keeps_the_clients_key_order() {
        let request = decode_request(CHOICE.as_bytes()).unwrap();
        assert_eq!(request.model, "laya");
        assert_eq!(
            request.question_text(),
            r#"{"state":{"z":1,"a":2},"questions":{"route":{"type":"choice","instructions":"Which team?","criteria":{"zeta":"last letter","alpha":"first letter","mid":"between"}}}}"#
        );
    }

    #[test]
    fn the_chat_payload_is_one_user_message_holding_the_question_text() {
        let request = decode_request(CHOICE.as_bytes()).unwrap();
        let payload = request.chat_payload();
        let messages = payload.as_object().unwrap()["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        let message = messages[0].as_object().unwrap();
        assert_eq!(message["role"].as_str(), Some("user"));
        assert_eq!(
            message["content"].as_str(),
            Some(request.question_text().as_str())
        );
    }

    #[test]
    fn a_null_state_is_a_state() {
        let body = r#"{"model":"laya","questions":{"q":{"type":"noul","instructions":"ok?"}},"state":null}"#;
        let request = decode_request(body.as_bytes()).unwrap();
        assert!(request.question_text().starts_with(r#"{"state":null,"#));
    }

    #[test]
    fn extra_body_members_are_ignored() {
        let body = r#"{"model":"laya","questions":{"q":{"type":"noul","instructions":"ok?"}},"state":"s","trace":true}"#;
        assert!(decode_request(body.as_bytes()).is_ok());
    }

    #[test]
    fn every_caller_fault_is_a_bad_request_that_says_what_is_wrong() {
        let cases = [
            ("not json", "JSON object"),
            (
                r#"{"questions":{"q":{"type":"noul","instructions":"i"}},"state":1}"#,
                "model is required",
            ),
            (r#"{"model":"laya","state":1}"#, "questions is required"),
            (
                r#"{"model":"laya","questions":{"q":{"type":"noul","instructions":"i"}}}"#,
                "state is required",
            ),
            (
                r#"{"model":"laya","questions":{},"state":1}"#,
                "at least one question",
            ),
            (
                r#"{"model":"laya","questions":[1],"state":1}"#,
                "at least one question",
            ),
            (
                r#"{"model":"laya","questions":{"q":7},"state":1}"#,
                "\"q\" must be an object",
            ),
            (
                r#"{"model":"laya","questions":{"q":{"type":"essay","instructions":"i"}},"state":1}"#,
                "needs a type",
            ),
            (
                r#"{"model":"laya","questions":{"q":{"type":"noul"}},"state":1}"#,
                "needs instructions",
            ),
            (
                r#"{"model":"laya","questions":{"q":{"type":"choice","instructions":"i"}},"state":1}"#,
                "criteria to choose between",
            ),
            (
                r#"{"model":"laya","questions":{"q":{"type":"choice","instructions":"i","criteria":{}}},"state":1}"#,
                "criteria to choose between",
            ),
            (
                r#"{"model":"laya","questions":{"q":{"type":"score","instructions":"i","criteria":{"a":"b"}}},"state":1}"#,
                "list of levels",
            ),
        ];
        for (body, reason) in cases {
            let error = decode_request(body.as_bytes()).unwrap_err();
            assert_eq!(error.kind, GatewayErrorKind::BadRequest, "{body}");
            assert!(error.message.contains(reason), "{body}: {}", error.message);
        }
    }

    #[test]
    fn an_envelope_goes_back_byte_for_byte() {
        let reply = r#"{"model": "rl-agent", "answers": {"route": {"probabilities": {"zeta": 0.5, "alpha": 0.5}}}, "usage": {"input_tokens": 9, "output_tokens": 0}}"#;
        assert_eq!(envelope(reply).unwrap(), reply.as_bytes());
    }

    #[test]
    fn a_reply_that_is_not_an_envelope_is_a_server_error() {
        for reply in [
            "Sure! The answer is infra.",
            r#"{"text":"hi"}"#,
            r#"{"answers":[]}"#,
        ] {
            let error = envelope(reply).unwrap_err();
            assert_eq!(error.kind, GatewayErrorKind::ServerError, "{reply}");
        }
    }
}
