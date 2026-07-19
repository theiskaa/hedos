//! The OpenAI speech handler end to end against a mock port: audio frames
//! accumulated into a WAV response, plus the input-guard rejections.

mod common;

use common::MockPort;
use gateway::handlers::GatewayHandling;
use gateway::handlers::speech::OpenAISpeechHandler;
use gateway::identity::GatewayIdentity;
use gateway::request::GatewayRequest;
use gateway::responder::{GatewayResponder, ResponsePart};
use gateway::scopes::GatewayScopes;
use kernel::capabilities::{AudioFrame, CapabilityChunk};
use kernel::records::Capability;

fn identity() -> GatewayIdentity {
    GatewayIdentity::new("client", "Client", GatewayScopes::all())
}

fn request(body: &str) -> GatewayRequest {
    GatewayRequest::new(
        "POST",
        "/v1/audio/speech",
        Vec::new(),
        body.as_bytes().to_vec(),
    )
}

struct Response {
    status: Option<u16>,
    content_type: Option<String>,
    body: Vec<u8>,
}

fn collect(mut rx: tokio::sync::mpsc::UnboundedReceiver<ResponsePart>) -> Response {
    let mut response = Response {
        status: None,
        content_type: None,
        body: Vec::new(),
    };
    while let Ok(part) = rx.try_recv() {
        match part {
            ResponsePart::Head { status, headers } => {
                response.status = Some(status);
                response.content_type = headers
                    .into_iter()
                    .find(|(name, _)| name == "Content-Type")
                    .map(|(_, value)| value);
            }
            ResponsePart::Chunk(bytes) => response.body.extend(bytes),
        }
    }
    response
}

fn speech_port() -> MockPort {
    let (mut port, _id) = MockPort::with_capable_model("kokoro", Capability::speak());
    port.chunks = vec![
        CapabilityChunk::Audio(AudioFrame::new(vec![0, 0, 128, 63], 16_000)),
        CapabilityChunk::Done(None),
    ];
    port
}

#[tokio::test]
async fn audio_frames_become_a_wav_file() {
    let port = speech_port();
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"kokoro","input":"hello"}"#;
    OpenAISpeechHandler
        .handle(&request(body), &identity(), &port, &responder)
        .await
        .unwrap();
    let response = collect(rx);
    assert_eq!(response.status, Some(200));
    assert_eq!(response.content_type.as_deref(), Some("audio/wav"));
    assert_eq!(&response.body[0..4], b"RIFF");
    assert_eq!(&response.body[8..12], b"WAVE");
    // The sample rate lands in the `fmt ` chunk at byte offset 24.
    let rate = u32::from_le_bytes(response.body[24..28].try_into().unwrap());
    assert_eq!(rate, 16_000);
}

#[tokio::test]
async fn a_missing_input_is_rejected() {
    let port = speech_port();
    let (responder, rx) = GatewayResponder::new();
    let error = OpenAISpeechHandler
        .handle(
            &request(r#"{"model":"kokoro"}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("input is required"));
    let _ = collect(rx);
}

#[tokio::test]
async fn a_non_wav_format_is_rejected() {
    let port = speech_port();
    let (responder, rx) = GatewayResponder::new();
    let error = OpenAISpeechHandler
        .handle(
            &request(r#"{"model":"kokoro","input":"hi","response_format":"mp3"}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("wav"));
    let _ = collect(rx);
}

#[tokio::test]
async fn a_silent_stream_reports_no_audio() {
    let (mut port, _id) = MockPort::with_capable_model("kokoro", Capability::speak());
    port.chunks = vec![CapabilityChunk::Done(None)];
    let (responder, rx) = GatewayResponder::new();
    let error = OpenAISpeechHandler
        .handle(
            &request(r#"{"model":"kokoro","input":"hi"}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("produced no audio"));
    let _ = collect(rx);
}

#[tokio::test]
async fn a_default_voice_is_taken_from_the_port() {
    let mut port = speech_port();
    port.voices = vec!["nova".to_owned()];
    let (responder, rx) = GatewayResponder::new();
    OpenAISpeechHandler
        .handle(
            &request(r#"{"model":"kokoro","input":"hi"}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap();
    assert_eq!(collect(rx).status, Some(200));
}
