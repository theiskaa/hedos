//! The chat handlers end to end against a mock port: streaming and unary, on both
//! the OpenAI and Ollama surfaces, plus the tool-support guard.

mod common;

use common::MockPort;
use gateway::error::GatewayErrorKind;
use gateway::handlers::GatewayHandling;
use gateway::handlers::chat::{OllamaChatHandler, OpenAIChatHandler};
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

fn hello_port() -> MockPort {
    let (mut port, _id) = MockPort::with_ready_model("llama3");
    port.chunks = vec![
        CapabilityChunk::Text("Hello".to_owned()),
        CapabilityChunk::Text(" world".to_owned()),
        CapabilityChunk::Done(Some(GenerationStats {
            prompt_tokens: Some(2),
            completion_tokens: Some(2),
            ..Default::default()
        })),
    ];
    port
}

#[tokio::test]
async fn openai_streaming_chat_emits_sse_frames() {
    let port = hello_port();
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"llama3","messages":[{"role":"user","content":"hi"}],"stream":true}"#;
    OpenAIChatHandler::default()
        .handle(
            &request("/v1/chat/completions", body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap();
    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    assert!(out.contains("chat.completion.chunk"));
    assert!(out.contains("Hello"));
    assert!(out.contains("world"));
    assert!(out.contains("\"finish_reason\":\"stop\""));
    assert!(out.trim_end().ends_with("data: [DONE]"));
}

#[tokio::test]
async fn openai_unary_chat_returns_one_completion() {
    let port = hello_port();
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"llama3","messages":[{"role":"user","content":"hi"}]}"#;
    OpenAIChatHandler::default()
        .handle(
            &request("/v1/chat/completions", body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap();
    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["object"], "chat.completion");
    assert_eq!(value["choices"][0]["message"]["content"], "Hello world");
    assert_eq!(value["usage"]["total_tokens"], 4);
}

#[tokio::test]
async fn ollama_streaming_chat_emits_ndjson() {
    let port = hello_port();
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"llama3","messages":[{"role":"user","content":"hi"}]}"#;
    OllamaChatHandler::default()
        .handle(&request("/api/chat", body), &identity(), &port, &responder)
        .await
        .unwrap();
    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    // NDJSON: one object per line; the last is the done frame.
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.iter().any(|line| line.contains("Hello")));
    let last: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    assert_eq!(last["done"], true);
    assert_eq!(last["done_reason"], "stop");
}

#[tokio::test]
async fn ollama_unary_chat_returns_one_object() {
    let mut port = hello_port();
    // Force unary.
    let (responder, rx) = GatewayResponder::new();
    port.chunks = vec![
        CapabilityChunk::Text("Hi".to_owned()),
        CapabilityChunk::Done(None),
    ];
    let body = r#"{"model":"llama3","messages":[{"role":"user","content":"hi"}],"stream":false}"#;
    OllamaChatHandler::default()
        .handle(&request("/api/chat", body), &identity(), &port, &responder)
        .await
        .unwrap();
    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["message"]["content"], "Hi");
    assert_eq!(value["done"], true);
}

#[tokio::test]
async fn a_tool_request_to_a_non_tool_model_is_rejected() {
    let port = hello_port(); // supports_tools defaults to false
    let (responder, _rx) = GatewayResponder::new();
    let body = r#"{"model":"llama3","messages":[{"role":"user","content":"hi"}],"tools":[{"type":"function","function":{"name":"add"}}]}"#;
    let error = OpenAIChatHandler::default()
        .handle(
            &request("/v1/chat/completions", body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, GatewayErrorKind::BadRequest);
    assert!(error.message.contains("does not support tool calling"));
}
