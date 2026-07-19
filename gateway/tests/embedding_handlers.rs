//! The embedding handlers end to end against a mock port: OpenAI `/v1/embeddings`
//! (float and base64) and Ollama `/api/embed` plus the legacy `/api/embeddings`.

mod common;

use common::MockPort;
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
