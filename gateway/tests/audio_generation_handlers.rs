//! `/v1/audio/generations` end to end against a mock port: a finished job's
//! file returned whole or streamed as server-sent events, a request checked
//! against the model's parameters before anything runs, a refusal the runtime
//! blames on the request answered as a `400`, and a client that goes away
//! cancelling the job. Also the speech route's refusal of a generator.

mod common;

use std::sync::Mutex;

use base64::prelude::{BASE64_STANDARD, Engine as _};
use common::{MockPort, capable_model};
use gateway::handlers::GatewayHandling;
use gateway::handlers::audio_generation::AudioGenerationHandler;
use gateway::handlers::speech::OpenAISpeechHandler;
use gateway::identity::GatewayIdentity;
use gateway::request::GatewayRequest;
use gateway::responder::{GatewayResponder, ResponsePart};
use gateway::router::standard_routes;
use gateway::scopes::GatewayScopes;
use kernel::jobs::{JobEvent, JobProgress};
use kernel::records::{Capability, JsonValue, Modality, ParamSpec, ParamType};

fn identity() -> GatewayIdentity {
    GatewayIdentity::new("client", "Client", GatewayScopes::all())
}

fn request(path: &str, body: &str) -> GatewayRequest {
    GatewayRequest::new("POST", path, Vec::new(), body.as_bytes().to_vec())
}

fn generation(body: &str) -> GatewayRequest {
    request("/v1/audio/generations", body)
}

/// The status, headers and body a responder produced.
fn collect(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<ResponsePart>,
) -> (Option<u16>, Vec<(String, String)>, Vec<u8>) {
    let mut status = None;
    let mut headers = Vec::new();
    let mut body = Vec::new();
    while let Ok(part) = rx.try_recv() {
        match part {
            ResponsePart::Head {
                status: code,
                headers: sent,
            } => {
                status = Some(code);
                headers = sent;
            }
            ResponsePart::Chunk(bytes) => body.extend(bytes),
        }
    }
    (status, headers, body)
}

fn content_type(headers: &[(String, String)]) -> Option<&str> {
    headers
        .iter()
        .find(|(name, _)| name == "Content-Type")
        .map(|(_, value)| value.as_str())
}

/// A ready Stable Audio-like model: a duration of 1-47 s, steps and a seed.
fn stable_audio_port(events: Vec<JobEvent>) -> MockPort {
    let mut record = capable_model("stable-audio", Capability::audio());
    record.modality = Modality::audio();
    record.params = vec![
        ParamSpec {
            key: "duration".to_owned(),
            param_type: ParamType::Float,
            default_value: None,
            range: Some(vec![JsonValue::Double(1.0), JsonValue::Double(47.0)]),
            values: None,
        },
        ParamSpec {
            key: "seed".to_owned(),
            param_type: ParamType::Int,
            default_value: None,
            range: None,
            values: None,
        },
    ];
    let mut port = MockPort {
        shelf: vec![record],
        job_events: events,
        ..MockPort::default()
    };
    port.artifacts
        .insert("art-1".to_owned(), b"fLaC-song".to_vec());
    port
}

fn finished() -> Vec<JobEvent> {
    vec![
        JobEvent::Running,
        JobEvent::Status("Synthesizing".to_owned()),
        JobEvent::Progress(JobProgress::new(0.5, Some(50), Some(100))),
        JobEvent::Done {
            result: vec!["art-1".to_owned()],
        },
    ]
}

#[tokio::test]
async fn a_finished_job_answers_with_the_file() {
    let port = stable_audio_port(finished());
    let (responder, rx) = GatewayResponder::new();
    AudioGenerationHandler
        .handle(
            &generation(
                r#"{"model":"stable-audio","prompt":"rain","duration_seconds":12,"seed":3}"#,
            ),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap();
    let (status, headers, body) = collect(rx);
    assert_eq!(status, Some(200));
    assert_eq!(content_type(&headers), Some("audio/flac"));
    assert_eq!(body, b"fLaC-song");

    let submitted = port.submitted.lock().unwrap();
    let (capability, payload) = &submitted[0];
    assert_eq!(capability, &Capability::audio());
    let payload = payload.as_object().unwrap();
    assert_eq!(payload.get("duration"), Some(&JsonValue::Double(12.0)));
    assert_eq!(payload.get("seed"), Some(&JsonValue::Int(3)));
    assert_eq!(
        payload.get("format"),
        Some(&JsonValue::String("flac".into()))
    );
}

#[tokio::test]
async fn wav_is_answered_as_wav() {
    let port = stable_audio_port(finished());
    let (responder, rx) = GatewayResponder::new();
    AudioGenerationHandler
        .handle(
            &generation(r#"{"model":"stable-audio","prompt":"rain","response_format":"wav"}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap();
    let (_, headers, _) = collect(rx);
    assert_eq!(content_type(&headers), Some("audio/wav"));
}

#[tokio::test]
async fn what_the_model_does_not_take_is_refused_before_anything_runs() {
    for (body, says) in [
        (
            r#"{"model":"stable-audio","prompt":"rain","lyrics":"la la"}"#,
            "does not take lyrics",
        ),
        (
            r#"{"model":"stable-audio","prompt":"rain","bpm":120}"#,
            "does not take bpm",
        ),
        (
            r#"{"model":"stable-audio","prompt":"rain","duration":600}"#,
            "between 1 and 47",
        ),
        (
            r#"{"model":"stable-audio","prompt":"rain","response_format":"mp3"}"#,
            "not mp3",
        ),
        (r#"{"model":"stable-audio"}"#, "prompt is required"),
        (
            r#"{"model":"stable-audio","prompt":"r","duration":5,"duration_seconds":5}"#,
            "given twice",
        ),
    ] {
        let port = stable_audio_port(finished());
        let (responder, _rx) = GatewayResponder::new();
        let error = AudioGenerationHandler
            .handle(&generation(body), &identity(), &port, &responder)
            .await
            .expect_err(body);
        assert_eq!(error.status(), 400, "{body}");
        assert!(error.to_string().contains(says), "{body}: {error}");
        assert!(port.submitted.lock().unwrap().is_empty(), "{body}");
    }
}

#[tokio::test]
async fn a_stream_reports_progress_then_the_file() {
    let port = stable_audio_port(finished());
    let (responder, rx) = GatewayResponder::new();
    AudioGenerationHandler
        .handle(
            &generation(r#"{"model":"stable-audio","prompt":"rain","stream":true}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap();
    let (status, headers, body) = collect(rx);
    assert_eq!(status, Some(200));
    assert_eq!(content_type(&headers), Some("text/event-stream"));
    let text = String::from_utf8(body).unwrap();
    let names: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("event: "))
        .collect();
    assert_eq!(names, ["progress", "progress", "progress", "complete"]);
    assert!(text.contains(r#""stage":"Synthesizing""#), "{text}");
    assert!(text.contains(r#""step":50"#), "{text}");
    let complete = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .next_back()
        .unwrap();
    let complete: serde_json::Value = serde_json::from_str(complete).unwrap();
    assert_eq!(complete["b64_json"], BASE64_STANDARD.encode(b"fLaC-song"));
    assert_eq!(complete["format"], "flac");
}

#[tokio::test]
async fn a_request_the_runtime_refuses_is_a_bad_request() {
    for (rejected, status) in [(true, 400), (false, 500)] {
        let port = stable_audio_port(vec![JobEvent::Failed {
            message: "YuE2 needs lyrics".to_owned(),
            rejected,
        }]);
        let (responder, _rx) = GatewayResponder::new();
        let error = AudioGenerationHandler
            .handle(
                &generation(r#"{"model":"stable-audio","prompt":"pop"}"#),
                &identity(),
                &port,
                &responder,
            )
            .await
            .expect_err("failed");
        assert_eq!(error.status(), status);
        assert!(error.to_string().contains("needs lyrics"));
    }
}

#[tokio::test]
async fn a_failure_while_streaming_ends_with_an_error_event_and_is_the_outcome() {
    let port = stable_audio_port(vec![
        JobEvent::Running,
        JobEvent::Failed {
            message: "out of memory".to_owned(),
            rejected: false,
        },
    ]);
    let (responder, rx) = GatewayResponder::new();
    let error = AudioGenerationHandler
        .handle(
            &generation(r#"{"model":"stable-audio","prompt":"x","stream":true}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .expect_err("the failure is the request's outcome, for the audit");
    assert_eq!(error.status(), 500);
    let text = String::from_utf8(collect(rx).2).unwrap();
    assert!(text.contains("event: error\ndata: "), "{text}");
    assert!(text.contains("out of memory"), "{text}");
}

#[tokio::test]
async fn an_artifact_gone_after_the_job_finished_is_an_error_event() {
    let mut port = stable_audio_port(finished());
    port.artifacts.clear();
    let (responder, rx) = GatewayResponder::new();
    let error = AudioGenerationHandler
        .handle(
            &generation(r#"{"model":"stable-audio","prompt":"x","stream":true}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .expect_err("no audio");
    assert!(error.to_string().contains("produced no audio"), "{error}");
    let text = String::from_utf8(collect(rx).2).unwrap();
    assert!(text.contains("event: error"), "{text}");
}

#[tokio::test]
async fn a_finished_job_is_never_cancelled() {
    let port = stable_audio_port(finished());
    let (responder, _rx) = GatewayResponder::new();
    AudioGenerationHandler
        .handle(
            &generation(r#"{"model":"stable-audio","prompt":"x"}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .unwrap();
    assert!(port.cancelled.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_request_cut_short_cancels_its_job_once() {
    let mut port = stable_audio_port(vec![JobEvent::Running]);
    port.open_jobs = Some(Mutex::new(Vec::new()));
    let (responder, _rx) = GatewayResponder::new();
    let request = generation(r#"{"model":"stable-audio","prompt":"x","stream":true}"#);
    let identity = identity();
    let handling = AudioGenerationHandler.handle(&request, &identity, &port, &responder);
    // The server drops the dispatch when it cuts a request; so does this.
    let cut = tokio::time::timeout(std::time::Duration::from_millis(50), handling).await;
    assert!(cut.is_err(), "still running when cut");
    assert_eq!(*port.cancelled.lock().unwrap(), ["job-1"]);
}

#[tokio::test]
async fn a_client_that_goes_away_cancels_the_job() {
    let mut port = stable_audio_port(vec![JobEvent::Running]);
    port.open_jobs = Some(Mutex::new(Vec::new()));
    let (responder, rx) = GatewayResponder::new();
    drop(rx);
    let error = AudioGenerationHandler
        .handle(
            &generation(r#"{"model":"stable-audio","prompt":"x"}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .expect_err("abandoned");
    assert!(error.to_string().contains("went away"), "{error}");
    assert_eq!(
        error.status(),
        499,
        "a client that left, not a server error"
    );
    assert_eq!(*port.cancelled.lock().unwrap(), ["job-1"]);
}

#[tokio::test]
async fn a_voice_is_pointed_at_the_speech_route_and_a_generator_at_this_one() {
    let (port, _) = MockPort::with_capable_model("kokoro", Capability::speak());
    let (responder, _rx) = GatewayResponder::new();
    let error = AudioGenerationHandler
        .handle(
            &generation(r#"{"model":"kokoro","prompt":"x"}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .expect_err("a voice");
    assert_eq!(error.status(), 400);
    assert!(error.to_string().contains("/v1/audio/speech"), "{error}");

    let port = stable_audio_port(finished());
    let (responder, _rx) = GatewayResponder::new();
    let error = OpenAISpeechHandler
        .handle(
            &request(
                "/v1/audio/speech",
                r#"{"model":"stable-audio","input":"hi"}"#,
            ),
            &identity(),
            &port,
            &responder,
        )
        .await
        .expect_err("a generator");
    assert_eq!(error.status(), 400);
    assert!(
        error.to_string().contains("/v1/audio/generations"),
        "{error}"
    );
}

#[test]
fn the_route_answers_under_mlx_serves_name_too() {
    let paths: Vec<String> = standard_routes()
        .into_iter()
        .filter(|route| route.method == "POST")
        .map(|route| route.path)
        .collect();
    assert!(paths.contains(&"/v1/audio/generations".to_owned()));
    assert!(paths.contains(&"/v1/audio/music-generations".to_owned()));
}
