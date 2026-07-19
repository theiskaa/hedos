//! The model-listing handlers against a mock port: `/v1/models`, `/api/tags`,
//! `/api/version`, `/api/show`.

mod common;

use common::MockPort;
use gateway::handlers::GatewayHandling;
use gateway::handlers::models::{
    OllamaShowHandler, OllamaTagsHandler, OllamaVersionHandler, OpenAIModelsHandler,
};
use gateway::identity::GatewayIdentity;
use gateway::request::GatewayRequest;
use gateway::responder::{GatewayResponder, ResponsePart};
use gateway::scopes::GatewayScopes;

fn identity() -> GatewayIdentity {
    GatewayIdentity::new("client", "Client", GatewayScopes::all())
}

fn get(uri: &str) -> GatewayRequest {
    GatewayRequest::new("GET", uri, Vec::new(), Vec::new())
}

fn body(rx: tokio::sync::mpsc::UnboundedReceiver<ResponsePart>) -> serde_json::Value {
    let mut rx = rx;
    let mut bytes = Vec::new();
    while let Ok(part) = rx.try_recv() {
        if let ResponsePart::Chunk(chunk) = part {
            bytes.extend(chunk);
        }
    }
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn openai_models_lists_ready_models() {
    let (port, _id) = MockPort::with_ready_model("llama3");
    let (responder, rx) = GatewayResponder::new();
    OpenAIModelsHandler
        .handle(&get("/v1/models"), &identity(), &port, &responder)
        .await
        .unwrap();
    let value = body(rx);
    assert_eq!(value["object"], "list");
    assert_eq!(value["data"][0]["id"], "llama3");
    assert_eq!(value["data"][0]["owned_by"], "hedos");
}

#[tokio::test]
async fn ollama_tags_lists_ready_chat_models() {
    let (port, _id) = MockPort::with_ready_model("llama3");
    let (responder, rx) = GatewayResponder::new();
    OllamaTagsHandler
        .handle(&get("/api/tags"), &identity(), &port, &responder)
        .await
        .unwrap();
    let value = body(rx);
    assert_eq!(value["models"][0]["name"], "llama3");
}

#[tokio::test]
async fn ollama_version_is_reported() {
    let port = MockPort::default();
    let (responder, rx) = GatewayResponder::new();
    OllamaVersionHandler
        .handle(&get("/api/version"), &identity(), &port, &responder)
        .await
        .unwrap();
    assert_eq!(body(rx)["version"], "0.5.0");
}

#[tokio::test]
async fn ollama_show_reports_capabilities() {
    let (port, _id) = MockPort::with_ready_model("llama3");
    let (responder, rx) = GatewayResponder::new();
    let request = GatewayRequest::new(
        "POST",
        "/api/show",
        Vec::new(),
        br#"{"model":"llama3"}"#.to_vec(),
    );
    OllamaShowHandler
        .handle(&request, &identity(), &port, &responder)
        .await
        .unwrap();
    let value = body(rx);
    assert!(
        value["capabilities"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("completion"))
    );
    assert!(value["details"].is_object());
}

#[tokio::test]
async fn ollama_show_requires_a_model() {
    let (port, _id) = MockPort::with_ready_model("llama3");
    let (responder, _rx) = GatewayResponder::new();
    let request = GatewayRequest::new("POST", "/api/show", Vec::new(), b"{}".to_vec());
    assert!(
        OllamaShowHandler
            .handle(&request, &identity(), &port, &responder)
            .await
            .is_err()
    );
}
