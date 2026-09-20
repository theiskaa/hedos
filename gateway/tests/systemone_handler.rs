//! TypeSafe's `/v1/systemone` end to end through the router against a mock
//! port: the envelope a TypeSafe SDK decodes, option order carried untouched to
//! the model, the refusals that name which models would do, and caller faults
//! answered as `400`s in the error shape the SDK reads.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::{MockPort, capable_model, ready_model};
use gateway::audit::NoopAudit;
use gateway::auth::OpenAuth;
use gateway::port::GatewayPort;
use gateway::request::GatewayRequest;
use gateway::responder::{GatewayResponder, ResponsePart};
use gateway::router::{GatewayRouter, standard_routes};
use kernel::capabilities::CapabilityChunk;
use kernel::records::{Capability, JsonValue, ModelRecord};
use serde::Deserialize;

/// The reply laya's sidecar writes for the routing question, as it writes it.
const LAYA_REPLY: &str = r#"{"model": "rl-agent", "answers": {"route": {"type": "choice", "choice": "infra", "probabilities": {"infra": 0.9037, "billing": 0.0466, "design": 0.0498}, "confidence": 0.6508, "rl_agent": {"act_probability": 1.0}}}, "usage": {"input_tokens": 41, "output_tokens": 0}}"#;

const ROUTING_QUESTION: &str = r#"{"model":"laya","questions":{"route":{"type":"choice","instructions":"Which team should handle this?","criteria":{"infra":"servers and networking","billing":"payments","design":"visual and UX issues"}}},"state":"The server returned 500 three times in a row."}"#;

/// The TypeSafe Swift SDK's `Envelope`, `ChoiceAnswer` and `Usage`, field for
/// field, so a body that decodes here decodes there.
#[derive(Deserialize)]
struct Envelope {
    model: String,
    answers: BTreeMap<String, ChoiceAnswer>,
    usage: Usage,
}

#[derive(Deserialize)]
struct ChoiceAnswer {
    choice: String,
    confidence: f64,
    probabilities: BTreeMap<String, f64>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

fn laya() -> ModelRecord {
    let mut record = capable_model("laya", Capability::chat());
    record.capabilities.push(Capability::judge());
    record
}

fn port(shelf: Vec<ModelRecord>, reply: &str) -> Arc<MockPort> {
    Arc::new(MockPort {
        shelf,
        chunks: vec![
            CapabilityChunk::Status("generating".to_owned()),
            CapabilityChunk::Text(reply.to_owned()),
            CapabilityChunk::Done(None),
        ],
        ..MockPort::default()
    })
}

struct Response {
    status: Option<u16>,
    headers: Vec<(String, String)>,
    body: String,
}

/// Post `body` to `/v1/systemone` the way the SDK does, bearer token included.
async fn post(port: &Arc<MockPort>, body: &str) -> Response {
    let router = GatewayRouter::new(
        Arc::clone(port) as Arc<dyn GatewayPort>,
        Box::new(OpenAuth),
        Box::new(NoopAudit),
        standard_routes(),
        4,
    );
    let headers = vec![
        (
            "Authorization".to_owned(),
            "Bearer ts-a-real-typesafe-key".to_owned(),
        ),
        ("Content-Type".to_owned(), "application/json".to_owned()),
    ];
    let request = GatewayRequest::new("POST", "/v1/systemone", headers, body.as_bytes().to_vec());
    let (responder, mut rx) = GatewayResponder::new();
    router.dispatch(request, &responder).await;
    let mut response = Response {
        status: None,
        headers: Vec::new(),
        body: String::new(),
    };
    while let Ok(part) = rx.try_recv() {
        match part {
            ResponsePart::Head { status, headers } => {
                response.status = Some(status);
                response.headers = headers;
            }
            ResponsePart::Chunk(bytes) => response.body.push_str(&String::from_utf8_lossy(&bytes)),
        }
    }
    response
}

/// The text of a failure as the SDK would show it: `error.message`.
fn error_message(response: &Response) -> String {
    let value: serde_json::Value = serde_json::from_str(&response.body).expect("a JSON error body");
    value["error"]["message"]
        .as_str()
        .expect("error.message")
        .to_owned()
}

#[tokio::test]
async fn a_choice_round_trips_into_the_envelope_the_sdk_decodes() {
    let port = port(vec![laya()], LAYA_REPLY);
    let response = post(&port, ROUTING_QUESTION).await;

    assert_eq!(response.status, Some(200), "{}", response.body);
    assert_eq!(
        response.body, LAYA_REPLY,
        "the model's reply, byte for byte"
    );
    let envelope: Envelope = serde_json::from_str(&response.body).expect("the SDK's envelope");
    assert_eq!(envelope.model, "rl-agent");
    assert_eq!(envelope.answers["route"].choice, "infra");
    assert_eq!(envelope.answers["route"].confidence, 0.6508);
    assert_eq!(envelope.answers["route"].probabilities["infra"], 0.9037);
    assert_eq!(envelope.usage.input_tokens, 41);
    assert_eq!(envelope.usage.output_tokens, 0);
    assert!(
        response
            .headers
            .iter()
            .any(|(name, value)| name == "x-typesafe-request-id" && value.starts_with("req-")),
        "{:?}",
        response.headers
    );
}

#[tokio::test]
async fn option_order_reaches_the_model_as_the_client_wrote_it() {
    // Deliberately not alphabetical: a parse-and-reserialize would sort these.
    let body = r#"{"model":"laya","questions":{"zebra":{"type":"choice","instructions":"Pick","criteria":{"yes":"y","no":"n","maybe":"m"}},"apple":{"type":"noul","instructions":"ok?"}},"state":{"z":1,"a":2}}"#;
    let port = port(vec![laya()], LAYA_REPLY);
    assert_eq!(post(&port, body).await.status, Some(200));

    let invoked = port.invoked.lock().unwrap();
    let (capability, payload) = invoked.first().expect("one invoke");
    assert_eq!(
        *capability,
        Capability::chat(),
        "the wire the sidecar speaks"
    );
    let messages = payload.as_object().unwrap()["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    let content = messages[0].as_object().unwrap()["content"]
        .as_str()
        .unwrap();
    assert_eq!(
        content,
        r#"{"state":{"z":1,"a":2},"questions":{"zebra":{"type":"choice","instructions":"Pick","criteria":{"yes":"y","no":"n","maybe":"m"}},"apple":{"type":"noul","instructions":"ok?"}}}"#
    );
    assert_eq!(
        messages[0].as_object().unwrap()["role"],
        JsonValue::String("user".to_owned())
    );
}

#[tokio::test]
async fn the_sdks_default_model_is_refused_with_the_models_that_would_do() {
    let port = port(vec![laya(), ready_model("llama3")], LAYA_REPLY);
    let response = post(
        &port,
        &ROUTING_QUESTION.replace("\"laya\"", "\"jev-latest\""),
    )
    .await;

    assert_eq!(response.status, Some(404));
    let message = error_message(&response);
    assert!(message.contains("jev-latest"), "{message}");
    assert!(
        message.ends_with("the models that answer typed questions here: laya"),
        "{message}"
    );
    assert!(
        port.invoked.lock().unwrap().is_empty(),
        "nothing else answered in its place"
    );
}

#[tokio::test]
async fn a_chat_model_is_refused_rather_than_asked() {
    let port = port(vec![laya(), ready_model("llama3")], "Sure! I think infra.");
    let response = post(&port, &ROUTING_QUESTION.replace("\"laya\"", "\"llama3\"")).await;

    assert_eq!(response.status, Some(400));
    let message = error_message(&response);
    assert!(
        message.starts_with("llama3 does not answer typed questions"),
        "{message}"
    );
    assert!(message.ends_with(": laya"), "{message}");
    assert!(port.invoked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_shelf_with_no_judge_says_so() {
    let port = port(vec![ready_model("llama3")], "");
    let response = post(&port, ROUTING_QUESTION).await;
    assert_eq!(response.status, Some(404));
    assert!(error_message(&response).ends_with("no model on this machine answers typed questions"));
}

#[tokio::test]
async fn a_body_missing_state_or_questions_is_a_readable_400_and_never_reaches_the_model() {
    let port = port(vec![laya()], LAYA_REPLY);
    let cases = [
        (
            r#"{"model":"laya","questions":{"q":{"type":"noul","instructions":"ok?"}}}"#,
            "state is required",
        ),
        (r#"{"model":"laya","state":"s"}"#, "questions is required"),
        (
            r#"{"model":"laya","questions":{},"state":"s"}"#,
            "at least one question",
        ),
        (
            r#"{"model":"laya","questions":{"q":{"type":"essay","instructions":"i"}},"state":"s"}"#,
            "needs a type of choice, score, or noul",
        ),
    ];
    for (body, reason) in cases {
        let response = post(&port, body).await;
        assert_eq!(response.status, Some(400), "{body}");
        let message = error_message(&response);
        assert!(message.contains(reason), "{body}: {message}");
    }
    assert!(port.invoked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_reply_that_is_not_an_envelope_is_the_gateways_fault() {
    let port = port(vec![laya()], "I would say infra, probably.");
    let response = post(&port, ROUTING_QUESTION).await;
    assert_eq!(response.status, Some(500));
    assert_eq!(
        error_message(&response),
        "the model's reply was not a System One envelope"
    );
}
