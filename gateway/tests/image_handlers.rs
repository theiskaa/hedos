//! The OpenAI image handler end to end against a mock port: a job whose result
//! artifact is returned inline as base64, plus the input-guard rejections.

mod common;

use base64::prelude::{BASE64_STANDARD, Engine as _};
use common::MockPort;
use gateway::handlers::GatewayHandling;
use gateway::handlers::images::OpenAIImagesHandler;
use gateway::identity::GatewayIdentity;
use gateway::request::GatewayRequest;
use gateway::responder::{GatewayResponder, ResponsePart};
use gateway::scopes::GatewayScopes;
use kernel::jobs::JobEvent;
use kernel::records::Capability;

fn identity() -> GatewayIdentity {
    GatewayIdentity::new("client", "Client", GatewayScopes::all())
}

fn request(body: &str) -> GatewayRequest {
    GatewayRequest::new(
        "POST",
        "/v1/images/generations",
        Vec::new(),
        body.as_bytes().to_vec(),
    )
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

fn image_port() -> MockPort {
    let (mut port, _id) = MockPort::with_capable_model("sdxl", Capability::image());
    port.job_events = vec![JobEvent::Done {
        result: vec!["art-1".to_owned()],
    }];
    port.artifacts.insert("art-1".to_owned(), vec![1, 2, 3, 4]);
    port
}

#[tokio::test]
async fn a_finished_job_returns_the_image_as_base64() {
    let port = image_port();
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"sdxl","prompt":"a koala"}"#;
    OpenAIImagesHandler
        .handle(&request(body), &identity(), &port, &responder)
        .await
        .unwrap();
    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        value["data"][0]["b64_json"],
        BASE64_STANDARD.encode([1, 2, 3, 4])
    );
    assert!(value["created"].is_number());
}

#[tokio::test]
async fn a_missing_prompt_is_rejected() {
    let port = image_port();
    let (responder, rx) = GatewayResponder::new();
    let error = OpenAIImagesHandler
        .handle(
            &request(r#"{"model":"sdxl"}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("prompt is required"));
    let _ = collect(rx);
}

#[tokio::test]
async fn more_than_one_image_is_rejected() {
    let port = image_port();
    let (responder, rx) = GatewayResponder::new();
    let error = OpenAIImagesHandler
        .handle(
            &request(r#"{"model":"sdxl","prompt":"a koala","n":2}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("only one image per request"));
    let _ = collect(rx);
}

#[tokio::test]
async fn a_url_response_format_is_rejected() {
    let port = image_port();
    let (responder, rx) = GatewayResponder::new();
    let error = OpenAIImagesHandler
        .handle(
            &request(r#"{"model":"sdxl","prompt":"a koala","response_format":"url"}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("b64_json"));
    let _ = collect(rx);
}

#[tokio::test]
async fn a_failed_job_surfaces_a_server_error() {
    let (mut port, _id) = MockPort::with_capable_model("sdxl", Capability::image());
    port.job_events = vec![JobEvent::Failed {
        message: "the renderer crashed".to_owned(),
    }];
    let (responder, rx) = GatewayResponder::new();
    let error = OpenAIImagesHandler
        .handle(
            &request(r#"{"model":"sdxl","prompt":"a koala"}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert_eq!(error.message, "the renderer crashed");
    let _ = collect(rx);
}

#[tokio::test]
async fn a_job_with_no_artifact_reports_no_image() {
    let (mut port, _id) = MockPort::with_capable_model("sdxl", Capability::image());
    port.job_events = vec![JobEvent::Done { result: Vec::new() }];
    let (responder, rx) = GatewayResponder::new();
    let error = OpenAIImagesHandler
        .handle(
            &request(r#"{"model":"sdxl","prompt":"a koala"}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("produced no image"));
    let _ = collect(rx);
}
