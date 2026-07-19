//! The full HTTP path: a real axum server over a mock port, driven with an HTTP
//! client. This is the end-to-end proof the gateway serves requests.

mod common;

use std::sync::Arc;

use common::MockPort;
use gateway::audit::NoopAudit;
use gateway::auth::OpenAuth;
use gateway::port::GatewayPort;
use gateway::router::{GatewayRouter, standard_routes};
use gateway::server;
use kernel::capabilities::CapabilityChunk;
use tokio::net::TcpListener;

async fn start(port: MockPort) -> String {
    let router = Arc::new(GatewayRouter::new(
        Arc::new(port) as Arc<dyn GatewayPort>,
        Box::new(OpenAuth),
        Box::new(NoopAudit),
        standard_routes(),
        4,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(server::serve(listener, router));
    format!("http://{addr}")
}

#[tokio::test]
async fn the_version_endpoint_answers_over_http() {
    let base = start(MockPort::default()).await;
    let response = reqwest::get(format!("{base}/api/version")).await.unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["version"], "0.5.0");
}

#[tokio::test]
async fn a_chat_completion_answers_over_http() {
    let (mut port, _id) = MockPort::with_ready_model("llama3");
    port.chunks = vec![
        CapabilityChunk::Text("hello".to_owned()),
        CapabilityChunk::Done(None),
    ];
    let base = start(port).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"llama3","messages":[{"role":"user","content":"hi"}],"stream":false}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "hello");
}

#[tokio::test]
async fn a_streaming_chat_answers_with_sse_over_http() {
    let (mut port, _id) = MockPort::with_ready_model("llama3");
    port.chunks = vec![
        CapabilityChunk::Text("a".to_owned()),
        CapabilityChunk::Text("b".to_owned()),
        CapabilityChunk::Done(None),
    ];
    let base = start(port).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .body(r#"{"model":"llama3","messages":[{"role":"user","content":"hi"}],"stream":true}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let text = response.text().await.unwrap();
    assert!(text.contains("chat.completion.chunk"));
    assert!(text.trim_end().ends_with("data: [DONE]"));
}

#[tokio::test]
async fn an_unknown_route_is_404_over_http() {
    let base = start(MockPort::default()).await;
    let response = reqwest::get(format!("{base}/nope")).await.unwrap();
    assert_eq!(response.status(), 404);
}
