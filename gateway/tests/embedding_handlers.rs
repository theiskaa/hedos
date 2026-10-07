//! The embedding handlers end to end against a mock port: OpenAI `/v1/embeddings`
//! (float and base64) and Ollama `/api/embed` plus the legacy `/api/embeddings`.

mod common;

use common::MockPort;
use gateway::error::GatewayErrorKind;
use gateway::handlers::GatewayHandling;
use gateway::handlers::embeddings::{OllamaEmbedHandler, OpenAIEmbeddingsHandler};
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

/// A port serving one model that returns `vectors` from an embed invoke.
fn embed_port(vectors: Vec<Vec<f64>>) -> MockPort {
    let (mut port, _id) = MockPort::with_ready_model("embedder");
    let mut chunks: Vec<CapabilityChunk> =
        vectors.into_iter().map(CapabilityChunk::Vector).collect();
    chunks.push(CapabilityChunk::Done(Some(GenerationStats {
        prompt_tokens: Some(7),
        ..Default::default()
    })));
    port.chunks = chunks;
    port
}

#[tokio::test]
async fn openai_embeddings_returns_a_float_vector_per_input() {
    let port = embed_port(vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"embedder","input":["a","b"]}"#;
    OpenAIEmbeddingsHandler
        .handle(
            &request("/v1/embeddings", body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap();
    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["object"], "list");
    assert_eq!(value["data"].as_array().unwrap().len(), 2);
    assert_eq!(value["data"][0]["index"], 0);
    assert_eq!(value["data"][1]["embedding"][0], 0.3);
    assert_eq!(value["usage"]["prompt_tokens"], 7);
}

#[tokio::test]
async fn openai_embeddings_encodes_base64_when_asked() {
    let port = embed_port(vec![vec![1.0]]);
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"embedder","input":"a","encoding_format":"base64"}"#;
    OpenAIEmbeddingsHandler
        .handle(
            &request("/v1/embeddings", body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap();
    let (_status, out) = collect(rx);
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    // 1.0f32 little-endian is 00 00 80 3F → base64 "AACAPw==".
    assert_eq!(value["data"][0]["embedding"], "AACAPw==");
}

#[tokio::test]
async fn openai_embeddings_rejects_dimensions() {
    let port = embed_port(vec![vec![0.1]]);
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"embedder","input":"a","dimensions":256}"#;
    let error = OpenAIEmbeddingsHandler
        .handle(
            &request("/v1/embeddings", body),
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
async fn openai_embeddings_fails_on_a_count_mismatch() {
    // Two inputs but the runtime returns only one vector.
    let port = embed_port(vec![vec![0.1]]);
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"embedder","input":["a","b"]}"#;
    let error = OpenAIEmbeddingsHandler
        .handle(
            &request("/v1/embeddings", body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("embeddings for"));
    let _ = collect(rx);
}

#[tokio::test]
async fn openai_embeddings_rejects_token_arrays_but_not_other_arrays() {
    let port = embed_port(vec![vec![0.1]]);
    // An integer (token) array gets the "token array" message.
    let (responder, rx) = GatewayResponder::new();
    let error = OpenAIEmbeddingsHandler
        .handle(
            &request("/v1/embeddings", r#"{"model":"embedder","input":[1,2,3]}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("token array"));
    let _ = collect(rx);

    // A float/mixed array is just "input is required" (not a token array).
    let (responder, rx) = GatewayResponder::new();
    let error = OpenAIEmbeddingsHandler
        .handle(
            &request(
                "/v1/embeddings",
                r#"{"model":"embedder","input":[1.5,2.5]}"#,
            ),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert_eq!(error.message, "input is required");
    let _ = collect(rx);
}

#[tokio::test]
async fn ollama_embed_returns_an_embeddings_array() {
    let port = embed_port(vec![vec![0.5, 0.6]]);
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"embedder","input":"a"}"#;
    OllamaEmbedHandler
        .handle(&request("/api/embed", body), &identity(), &port, &responder)
        .await
        .unwrap();
    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["model"], "embedder");
    assert_eq!(value["embeddings"][0][1], 0.6);
    assert_eq!(value["prompt_eval_count"], 7);
}

#[tokio::test]
async fn ollama_legacy_embeddings_returns_a_single_embedding() {
    let port = embed_port(vec![vec![0.5, 0.6]]);
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"embedder","prompt":"a"}"#;
    OllamaEmbedHandler
        .handle(
            &request("/api/embeddings", body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap();
    let (_status, out) = collect(rx);
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    // The legacy endpoint returns a flat `embedding`, not an `embeddings` array.
    assert_eq!(value["embedding"][0], 0.5);
    assert!(value.get("embeddings").is_none());
}

#[tokio::test]
async fn a_batch_over_the_limit_is_refused_before_any_work() {
    let port = embed_port(vec![vec![0.1]]);
    let inputs: Vec<String> = (0..2049).map(|n| format!("text {n}")).collect();
    for (uri, key) in [("/v1/embeddings", "input"), ("/api/embed", "input")] {
        let body = serde_json::json!({ "model": "embedder", key: inputs }).to_string();
        let (responder, _rx) = GatewayResponder::new();
        let request = request(uri, &body);
        let identity = identity();
        let handling = if uri == "/api/embed" {
            OllamaEmbedHandler.handle(&request, &identity, &port, &responder)
        } else {
            OpenAIEmbeddingsHandler.handle(&request, &identity, &port, &responder)
        };
        let error = handling.await.unwrap_err();
        assert_eq!(error.status(), 400, "{uri}");
        assert_eq!(
            error.message,
            "the input holds 2049 items, more than the 2048 one request may embed"
        );
    }
    assert!(port.invoked.lock().unwrap().is_empty());

    let at_limit: Vec<String> = (0..2048).map(|n| format!("text {n}")).collect();
    let body = serde_json::json!({ "model": "embedder", "input": at_limit }).to_string();
    let (responder, _rx) = GatewayResponder::new();
    let error = OpenAIEmbeddingsHandler
        .handle(
            &request("/v1/embeddings", &body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    // Past the limit check: the mock answers one vector for 2048 inputs.
    assert!(
        error.message.contains("embeddings for"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn a_client_gone_before_the_vectors_are_in_lets_the_batch_go() {
    let (mut port, _id) = MockPort::with_ready_model("embedder");
    port.chunks = vec![CapabilityChunk::Vector(vec![0.1])];
    port.open_streams = Some(std::sync::Mutex::new(Vec::new()));
    let (responder, rx) = GatewayResponder::new();
    drop(rx);
    let body = r#"{"model":"embedder","input":["a","b"]}"#;
    let error = OpenAIEmbeddingsHandler
        .handle(
            &request("/v1/embeddings", body),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, GatewayErrorKind::ClientClosed);
    assert_eq!(error.audit_outcome(), "cancelled");
    let open = port.open_streams.as_ref().unwrap().lock().unwrap();
    assert_eq!(open.len(), 1);
    assert!(open[0].is_closed(), "the runtime's stream was kept");
}
