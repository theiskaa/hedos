//! The text-generation handlers end to end against a mock port: OpenAI
//! `/v1/completions` (streaming and unary) and Ollama `/api/generate`.

mod common;

use common::MockPort;
use gateway::handlers::GatewayHandling;
use gateway::handlers::generate::{OllamaGenerateHandler, OpenAICompletionsHandler};
use gateway::identity::GatewayIdentity;
use gateway::request::GatewayRequest;
use gateway::responder::{GatewayResponder, ResponsePart};
use gateway::scopes::GatewayScopes;
use kernel::capabilities::{CapabilityChunk, GenerationStats};

fn identity() -> GatewayIdentity {
    GatewayIdentity::new("client", "Client", GatewayScopes::all())
}

fn request(uri: &str, body: &str) -> GatewayRequest {
    GatewayRequest::new("POST", uri, Vec::new(), body.as_bytes().to_vec())
}

fn collect(mut rx: tokio::sync::mpsc::UnboundedReceiver<ResponsePart>) -> (Option<u16>, String) {
    let mut status = None;
    let mut body = Vec::new();
    while let Ok(part) = rx.try_recv() {
        match part {
            ResponsePart::Head { status: code, .. } => status = Some(code),
            ResponsePart::Chunk(bytes) => body.extend(bytes),
        }
    }
    (status, String::from_utf8_lossy(&body).into_owned())
}

fn hi_port() -> MockPort {
    let (mut port, _id) = MockPort::with_ready_model("llama3");
    port.chunks = vec![
        CapabilityChunk::Text("Hi".to_owned()),
        CapabilityChunk::Text(" there".to_owned()),
        CapabilityChunk::Done(Some(GenerationStats {
            prompt_tokens: Some(3),
            completion_tokens: Some(2),
            ..Default::default()
        })),
    ];
    port
}

#[tokio::test]
async fn openai_completions_unary_returns_a_text_completion() {
    let port = hi_port();
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"llama3","prompt":"say hi"}"#;
    OpenAICompletionsHandler
        .handle(
            &request("/v1/completions", body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap();
    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["object"], "text_completion");
    assert_eq!(value["choices"][0]["text"], "Hi there");
    assert_eq!(value["choices"][0]["finish_reason"], "stop");
    assert_eq!(value["usage"]["total_tokens"], 5);
}

#[tokio::test]
async fn openai_completions_streams_text_completion_chunks() {
    let port = hi_port();
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"llama3","prompt":"say hi","stream":true}"#;
    OpenAICompletionsHandler
        .handle(
            &request("/v1/completions", body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap();
    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    assert!(out.contains("text_completion"));
    assert!(out.contains("\"text\":\"Hi\""));
    assert!(out.contains("\"finish_reason\":\"stop\""));
    assert!(out.trim_end().ends_with("data: [DONE]"));
}

#[tokio::test]
async fn openai_completions_rejects_best_of_over_one() {
    let port = hi_port();
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"llama3","prompt":"hi","best_of":2}"#;
    let error = OpenAICompletionsHandler
        .handle(
            &request("/v1/completions", body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code.as_deref(), Some("unsupported_parameter"));
    let _ = collect(rx);
}

#[tokio::test]
async fn openai_completions_rejects_a_multi_element_prompt_array() {
    let port = hi_port();
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"llama3","prompt":["a","b"]}"#;
    let error = OpenAICompletionsHandler
        .handle(
            &request("/v1/completions", body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("exactly one string"));
    let _ = collect(rx);
}

#[tokio::test]
async fn ollama_generate_streams_ndjson_deltas_then_a_final() {
    let port = hi_port();
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"llama3","prompt":"say hi"}"#;
    OllamaGenerateHandler
        .handle(
            &request("/api/generate", body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap();
    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    assert!(out.contains("\"response\":\"Hi\""));
    assert!(out.contains("\"done\":true"));
    assert!(out.contains("\"done_reason\":\"stop\""));
}

#[tokio::test]
async fn ollama_generate_unary_accumulates_the_response() {
    let port = hi_port();
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"llama3","prompt":"say hi","stream":false}"#;
    OllamaGenerateHandler
        .handle(
            &request("/api/generate", body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap();
    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["response"], "Hi there");
    assert_eq!(value["done"], true);
    assert_eq!(value["prompt_eval_count"], 3);
}
