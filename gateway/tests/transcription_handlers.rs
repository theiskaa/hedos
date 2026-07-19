//! The OpenAI transcription handler end to end against a mock port: a multipart
//! WAV upload transcribed to JSON or plain text, plus the guard rejections.

mod common;

use common::MockPort;
use gateway::handlers::GatewayHandling;
use gateway::handlers::transcriptions::OpenAITranscriptionsHandler;
use gateway::identity::GatewayIdentity;
use gateway::request::GatewayRequest;
use gateway::responder::{GatewayResponder, ResponsePart};
use gateway::scopes::GatewayScopes;
use kernel::capabilities::CapabilityChunk;
use kernel::records::Capability;

const BOUNDARY: &str = "BOUND";

fn identity() -> GatewayIdentity {
    GatewayIdentity::new("client", "Client", GatewayScopes::all())
}

/// A one-sample WAV the port's decoder accepts.
fn wav() -> Vec<u8> {
    runtime::audio::wav_from_pcm(&[0, 0, 128, 63], 16_000)
}

/// Build a multipart body from `(name, filename, bytes)` parts.
fn multipart(parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, filename, data) in parts {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        let disposition = match filename {
            Some(filename) => {
                format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"")
            }
            None => format!("Content-Disposition: form-data; name=\"{name}\""),
        };
        body.extend_from_slice(disposition.as_bytes());
        body.extend_from_slice(b"\r\n\r\n");
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn request(body: Vec<u8>) -> GatewayRequest {
    GatewayRequest::new(
        "POST",
        "/v1/audio/transcriptions",
        vec![(
            "Content-Type".to_owned(),
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )],
        body,
    )
}

fn collect(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<ResponsePart>,
) -> (Option<u16>, String, String) {
    let mut status = None;
    let mut content_type = String::new();
    let mut body = Vec::new();
    while let Ok(part) = rx.try_recv() {
        match part {
            ResponsePart::Head {
                status: code,
                headers,
            } => {
                status = Some(code);
                content_type = headers
                    .into_iter()
                    .find(|(name, _)| name == "Content-Type")
                    .map(|(_, value)| value)
                    .unwrap_or_default();
            }
            ResponsePart::Chunk(bytes) => body.extend(bytes),
        }
    }
    (
        status,
        content_type,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

fn transcribe_port() -> MockPort {
    let (mut port, _id) = MockPort::with_capable_model("whisper", Capability::transcribe());
    port.chunks = vec![
        CapabilityChunk::Text("hello".to_owned()),
        CapabilityChunk::Segment {
            text: " world".to_owned(),
            start_ms: 0,
            end_ms: 500,
        },
        CapabilityChunk::Done(None),
    ];
    port
}

#[tokio::test]
async fn a_wav_upload_transcribes_to_json() {
    let port = transcribe_port();
    let (responder, rx) = GatewayResponder::new();
    let body = multipart(&[("model", None, b"whisper"), ("file", Some("a.wav"), &wav())]);
    OpenAITranscriptionsHandler
        .handle(&request(body), &identity(), &port, &responder)
        .await
        .unwrap();
    let (status, content_type, out) = collect(rx);
    assert_eq!(status, Some(200));
    assert!(content_type.starts_with("application/json"));
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["text"], "hello world");
}

#[tokio::test]
async fn the_text_format_returns_plain_text() {
    let port = transcribe_port();
    let (responder, rx) = GatewayResponder::new();
    let body = multipart(&[
        ("model", None, b"whisper"),
        ("response_format", None, b"text"),
        ("file", Some("a.wav"), &wav()),
    ]);
    OpenAITranscriptionsHandler
        .handle(&request(body), &identity(), &port, &responder)
        .await
        .unwrap();
    let (status, content_type, out) = collect(rx);
    assert_eq!(status, Some(200));
    assert!(content_type.starts_with("text/plain"));
    assert_eq!(out, "hello world");
}

#[tokio::test]
async fn a_non_multipart_request_is_rejected() {
    let port = transcribe_port();
    let (responder, rx) = GatewayResponder::new();
    let plain = GatewayRequest::new(
        "POST",
        "/v1/audio/transcriptions",
        vec![("Content-Type".to_owned(), "application/json".to_owned())],
        b"{}".to_vec(),
    );
    let error = OpenAITranscriptionsHandler
        .handle(&plain, &identity(), &port, &responder)
        .await
        .unwrap_err();
    assert!(error.message.contains("multipart/form-data"));
    let _ = collect(rx);
}

#[tokio::test]
async fn a_missing_file_is_rejected() {
    let port = transcribe_port();
    let (responder, rx) = GatewayResponder::new();
    let body = multipart(&[("model", None, b"whisper")]);
    let error = OpenAITranscriptionsHandler
        .handle(&request(body), &identity(), &port, &responder)
        .await
        .unwrap_err();
    assert!(error.message.contains("file is required"));
    let _ = collect(rx);
}

#[tokio::test]
async fn an_unsupported_format_is_rejected() {
    let port = transcribe_port();
    let (responder, rx) = GatewayResponder::new();
    let body = multipart(&[
        ("model", None, b"whisper"),
        ("response_format", None, b"srt"),
        ("file", Some("a.wav"), &wav()),
    ]);
    let error = OpenAITranscriptionsHandler
        .handle(&request(body), &identity(), &port, &responder)
        .await
        .unwrap_err();
    assert_eq!(error.code.as_deref(), Some("unsupported_parameter"));
    assert!(error.message.contains("srt"));
    let _ = collect(rx);
}

#[tokio::test]
async fn an_unsupported_field_is_rejected() {
    let port = transcribe_port();
    let (responder, rx) = GatewayResponder::new();
    let body = multipart(&[
        ("model", None, b"whisper"),
        ("language", None, b"en"),
        ("file", Some("a.wav"), &wav()),
    ]);
    let error = OpenAITranscriptionsHandler
        .handle(&request(body), &identity(), &port, &responder)
        .await
        .unwrap_err();
    assert_eq!(error.code.as_deref(), Some("unsupported_parameter"));
    assert!(error.message.contains("language"));
    let _ = collect(rx);
}

#[tokio::test]
async fn a_non_wav_file_is_rejected() {
    let port = transcribe_port();
    let (responder, rx) = GatewayResponder::new();
    let body = multipart(&[
        ("model", None, b"whisper"),
        ("file", Some("a.txt"), b"not audio"),
    ]);
    let error = OpenAITranscriptionsHandler
        .handle(&request(body), &identity(), &port, &responder)
        .await
        .unwrap_err();
    assert!(error.message.contains("RIFF WAVE"));
    let _ = collect(rx);
}
