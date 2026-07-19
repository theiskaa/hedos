//! The router end to end: dispatch reaches the right handler, and 404/405/route
//! errors render through the responder.

mod common;

use std::sync::Arc;

use common::MockPort;
use gateway::audit::NoopAudit;
use gateway::auth::OpenAuth;
use gateway::port::GatewayPort;
use gateway::request::GatewayRequest;
use gateway::responder::{GatewayResponder, ResponsePart};
use gateway::router::{GatewayRouter, standard_routes};
use kernel::capabilities::CapabilityChunk;

fn router(port: MockPort) -> GatewayRouter {
    GatewayRouter::new(
        Arc::new(port) as Arc<dyn GatewayPort>,
        Box::new(OpenAuth),
        Box::new(NoopAudit),
        standard_routes(),
        4,
    )
}

fn collect(rx: tokio::sync::mpsc::UnboundedReceiver<ResponsePart>) -> (Option<u16>, String) {
    let mut rx = rx;
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

#[tokio::test]
async fn a_chat_request_reaches_the_chat_handler() {
    let (mut port, _id) = MockPort::with_ready_model("llama3");
    port.chunks = vec![
        CapabilityChunk::Text("hi".to_owned()),
        CapabilityChunk::Done(None),
    ];
    let (responder, rx) = GatewayResponder::new();
    let request = GatewayRequest::new(
        "POST",
        "/v1/chat/completions",
        Vec::new(),
        br#"{"model":"llama3","messages":[{"role":"user","content":"hi"}],"stream":false}"#
            .to_vec(),
    );
    router(port).dispatch(request, &responder).await;
    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    assert!(out.contains("chat.completion"));
}

#[tokio::test]
async fn models_are_listed_through_the_router() {
    let (port, _id) = MockPort::with_ready_model("llama3");
    let (responder, rx) = GatewayResponder::new();
    let request = GatewayRequest::new("GET", "/v1/models", Vec::new(), Vec::new());
    router(port).dispatch(request, &responder).await;
    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    assert!(out.contains("llama3"));
}

#[tokio::test]
async fn an_unknown_path_renders_404() {
    let (port, _id) = MockPort::with_ready_model("llama3");
    let (responder, rx) = GatewayResponder::new();
    let request = GatewayRequest::new("GET", "/nope", Vec::new(), Vec::new());
    router(port).dispatch(request, &responder).await;
    let (status, out) = collect(rx);
    assert_eq!(status, Some(404));
    assert!(out.contains("no route"));
}

#[tokio::test]
async fn a_wrong_method_renders_405() {
    let (port, _id) = MockPort::with_ready_model("llama3");
    let (responder, rx) = GatewayResponder::new();
    // /v1/chat/completions only answers POST.
    let request = GatewayRequest::new("GET", "/v1/chat/completions", Vec::new(), Vec::new());
    router(port).dispatch(request, &responder).await;
    let (status, _out) = collect(rx);
    assert_eq!(status, Some(405));
}

#[tokio::test]
async fn the_version_handshake_is_served() {
    let (responder, rx) = GatewayResponder::new();
    let request = GatewayRequest::new("GET", "/api/version", Vec::new(), Vec::new());
    router(MockPort::default())
        .dispatch(request, &responder)
        .await;
    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    assert!(out.contains("0.5.0"));
}
