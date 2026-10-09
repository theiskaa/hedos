//! `POST /v1/extract` end to end through the router against a mock port: the
//! extractor's reply returned as written, the request handed over without its
//! `model`, and the refusals that name which models would do.

mod common;

use std::sync::Arc;

use common::{MockPort, capable_model};
use gateway::audit::NoopAudit;
use gateway::auth::OpenAuth;
use gateway::port::GatewayPort;
use gateway::request::GatewayRequest;
use gateway::responder::{GatewayResponder, ResponsePart};
use gateway::router::{GatewayRouter, standard_routes};
use kernel::capabilities::CapabilityChunk;
use kernel::records::{Capability, ModelRecord};
use runtime::adapters::RuntimeError;

/// What Tessera writes for a `detect` of one email, as it writes it.
const TESSERA_REPLY: &str = r#"{"model":"tessera","operation":"detect","entities":[{"kind":"email","text":"jordan@acme.example","start":9,"end":28,"confidence":0.99,"review_recommended":false,"source":"rules","normalized":"jordan@acme.example"}]}"#;

fn tessera() -> ModelRecord {
    capable_model("tessera", Capability::extract())
}

fn port(shelf: Vec<ModelRecord>, reply: &str) -> Arc<MockPort> {
    Arc::new(MockPort {
        shelf,
        chunks: vec![
            CapabilityChunk::Text(reply.to_owned()),
            CapabilityChunk::Done(None),
        ],
        ..MockPort::default()
    })
}

async fn post(port: &Arc<MockPort>, body: &str) -> (Option<u16>, String) {
    let router = GatewayRouter::new(
        Arc::clone(port) as Arc<dyn GatewayPort>,
        Box::new(OpenAuth),
        Box::new(NoopAudit),
        standard_routes(),
        4,
    );
    let headers = vec![("Content-Type".to_owned(), "application/json".to_owned())];
    let request = GatewayRequest::new("POST", "/v1/extract", headers, body.as_bytes().to_vec());
    let (responder, mut rx) = GatewayResponder::new();
    router.dispatch(&request, &responder).await;
    let (mut status, mut text) = (None, String::new());
    while let Ok(part) = rx.try_recv() {
        match part {
            ResponsePart::Head { status: head, .. } => status = Some(head),
            ResponsePart::Chunk(bytes) => text.push_str(&String::from_utf8_lossy(&bytes)),
        }
    }
    (status, text)
}

fn error_message(body: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(body).expect("a JSON error body");
    value["error"]["message"]
        .as_str()
        .expect("error.message")
        .to_owned()
}

#[tokio::test]
async fn the_extractors_reply_comes_back_as_written() {
    let port = port(vec![tessera()], TESSERA_REPLY);
    let (status, body) = post(
        &port,
        r#"{"model":"tessera","operation":"detect","text":"Write to jordan@acme.example","kinds":["email"]}"#,
    )
    .await;
    assert_eq!(status, Some(200));
    assert_eq!(body, TESSERA_REPLY);
    let invoked = port.invoked.lock().unwrap();
    let (capability, payload) = &invoked[0];
    assert_eq!(*capability, Capability::extract());
    assert_eq!(
        serde_json::to_value(payload).unwrap(),
        serde_json::json!({"operation": "detect", "text": "Write to jordan@acme.example", "kinds": ["email"]}),
        "the request goes over as written, without its model"
    );
}

#[tokio::test]
async fn a_model_that_does_not_extract_is_refused_with_the_ones_that_do() {
    let port = port(
        vec![tessera(), capable_model("gemma", Capability::chat())],
        TESSERA_REPLY,
    );
    let (status, body) = post(
        &port,
        r#"{"model":"gemma","operation":"detect","text":"x"}"#,
    )
    .await;
    assert_eq!(status, Some(400));
    assert_eq!(
        error_message(&body),
        "gemma does not extract contacts; the models that extract contacts here: tessera"
    );
    let (status, body) = post(&port, r#"{"model":"nope","operation":"detect","text":"x"}"#).await;
    assert_eq!(status, Some(404));
    assert!(error_message(&body).ends_with("the models that extract contacts here: tessera"));
    let empty = self::port(Vec::new(), TESSERA_REPLY);
    let (_, body) = post(
        &empty,
        r#"{"model":"tessera","operation":"detect","text":"x"}"#,
    )
    .await;
    assert!(error_message(&body).ends_with("no model on this machine extracts contacts"));
}

#[tokio::test]
async fn a_body_without_a_model_or_not_an_object_is_the_callers_fault() {
    let port = port(vec![tessera()], TESSERA_REPLY);
    let (status, body) = post(&port, r#"{"operation":"detect","text":"x"}"#).await;
    assert_eq!(status, Some(400));
    assert_eq!(error_message(&body), "model is required");
    let (status, body) = post(&port, r#"["tessera"]"#).await;
    assert_eq!(status, Some(400));
    assert_eq!(error_message(&body), "request body must be a JSON object");
}

#[tokio::test]
async fn a_request_the_extractor_refuses_is_a_400_with_its_reason() {
    let port = Arc::new(MockPort {
        shelf: vec![tessera()],
        stream_error: Some(RuntimeError::Rejected(
            "invalid request: unknown field `colour`".to_owned(),
        )),
        ..MockPort::default()
    });
    let (status, body) = post(
        &port,
        r#"{"model":"tessera","operation":"detect","text":"x","colour":1}"#,
    )
    .await;
    assert_eq!(status, Some(400));
    assert!(
        error_message(&body).contains("unknown field `colour`"),
        "{body}"
    );
}

#[tokio::test]
async fn a_reply_that_is_not_one_json_object_is_a_server_error() {
    let port = port(vec![tessera()], "not json");
    let (status, body) = post(
        &port,
        r#"{"model":"tessera","operation":"detect","text":"x"}"#,
    )
    .await;
    assert_eq!(status, Some(500));
    assert_eq!(
        error_message(&body),
        "tessera's reply was not one JSON object"
    );
}
