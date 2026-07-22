//! Integration tests for `SidecarSupervisor`, driven by a real Python fake
//! sidecar (`tests/support/fake_sidecar.py`). These spawn processes and use real
//! time, so they run on the default multi-thread-capable runtime, not paused
//! time.

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use kernel::capabilities::CapabilityChunk;
use kernel::jobs::JobRuntimeEvent;
use kernel::records::JsonValue;
use runtime::sidecar::{SidecarError, SidecarSpec, SidecarStream, SidecarSupervisor};
use support::TempDir;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn script_path() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/fake_sidecar.py").to_owned()
}

fn spec(mode: &str) -> SidecarSpec {
    spec_with(mode, false, Duration::from_secs(10))
}

fn spec_with(mode: &str, cooperative: bool, grace: Duration) -> SidecarSpec {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut spec = SidecarSpec::new(
        format!("fake-{mode}-{unique}"),
        PathBuf::from("/usr/bin/env"),
        vec!["python3".to_owned(), script_path(), mode.to_owned()],
    );
    spec.ready_timeout = Duration::from_secs(15);
    spec.cooperative_cancel = cooperative;
    spec.cancel_grace_timeout = grace;
    spec
}

fn jstr(value: &str) -> JsonValue {
    JsonValue::String(value.to_owned())
}

fn jobj(fields: &[(&str, JsonValue)]) -> JsonValue {
    JsonValue::Object(
        fields
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn chat(content: &str) -> JsonValue {
    jobj(&[
        ("op", jstr("chat")),
        (
            "messages",
            JsonValue::Array(vec![jobj(&[
                ("role", jstr("user")),
                ("content", jstr(content)),
            ])]),
        ),
    ])
}

async fn collect_chunks(mut stream: SidecarStream<CapabilityChunk>) -> Vec<CapabilityChunk> {
    let mut out = Vec::new();
    while let Some(item) = stream.recv().await {
        if let Ok(chunk) = item {
            out.push(chunk);
        }
    }
    out
}

#[tokio::test]
async fn streams_a_vector_from_the_fake_sidecar() {
    let spec = spec("normal");
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    let stream = supervisor.request(
        &spec,
        jobj(&[("op", jstr("embed")), ("input", jstr("hedos"))]),
    );
    let vectors: Vec<Vec<f64>> = collect_chunks(stream)
        .await
        .into_iter()
        .filter_map(|chunk| match chunk {
            CapabilityChunk::Vector(values) => Some(values),
            _ => None,
        })
        .collect();
    assert_eq!(vectors.len(), 1);
    assert_eq!(vectors[0].first().copied(), Some(5.0));
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn streams_audio_with_the_handshake_sample_rate() {
    let spec = spec("normal");
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    let stream = supervisor.request(&spec, jobj(&[("op", jstr("speak"))]));
    let mut audio = Vec::new();
    let mut statuses = Vec::new();
    let mut stats = None;
    let chunks = collect_chunks(stream).await;
    for chunk in chunks {
        match chunk {
            CapabilityChunk::Audio(frame) => {
                assert_eq!(frame.sample_rate, 16_000);
                audio.push(frame.data);
            }
            CapabilityChunk::Status(message) => statuses.push(message),
            CapabilityChunk::Done(done) => stats = done,
            _ => {}
        }
    }
    assert_eq!(audio.len(), 3);
    assert_eq!(audio[0], vec![0u8; 640]);
    assert!(statuses.iter().any(|s| s == "generating"));
    assert_eq!(stats.and_then(|s| s.duration_ms), Some(120));
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn surfaces_a_crash_mid_request() {
    let spec = spec("crash-mid-request");
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    let mut stream = supervisor.request(&spec, jobj(&[("op", jstr("speak"))]));
    let mut audio = 0;
    let mut errored = false;
    while let Some(item) = stream.recv().await {
        match item {
            Ok(CapabilityChunk::Audio(_)) => audio += 1,
            Ok(_) => {}
            Err(_) => errored = true,
        }
    }
    assert_eq!(audio, 1);
    assert!(errored, "a mid-request crash surfaces as an error");
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn parses_token_counts_from_the_chat_done_event() {
    let spec = spec("normal");
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    let stream = supervisor.request(&spec, chat("abcdef"));
    let mut text = String::new();
    let mut stats = None;
    for chunk in collect_chunks(stream).await {
        match chunk {
            CapabilityChunk::Text(delta) => text.push_str(&delta),
            CapabilityChunk::Done(done) => stats = done,
            _ => {}
        }
    }
    assert_eq!(text, "abcdef!");
    let stats = stats.expect("done stats");
    assert_eq!(stats.prompt_tokens, Some(1));
    assert_eq!(stats.completion_tokens, Some(3));
    assert_eq!(stats.duration_ms, Some(200));
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn maps_a_tool_call_event_to_a_tool_call_chunk() {
    let spec = spec("normal");
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    let stream = supervisor.request(&spec, chat("tool"));
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut done = false;
    for chunk in collect_chunks(stream).await {
        match chunk {
            CapabilityChunk::Text(delta) => text.push_str(&delta),
            CapabilityChunk::ToolCall(call) => calls.push(call),
            CapabilityChunk::Done(_) => done = true,
            _ => {}
        }
    }
    assert_eq!(text, "calling ");
    // The nameless second event is dropped rather than surfaced.
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "read");
    assert_eq!(
        calls[0].arguments,
        JsonValue::Object(
            [("path".to_owned(), JsonValue::String("a".to_owned()))]
                .into_iter()
                .collect()
        )
    );
    assert!(calls[0].id.starts_with("call_"));
    assert!(done);
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn concurrent_stream_requests_serialize_without_corruption() {
    let spec = spec("normal");
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    let count_text = |chunks: Vec<CapabilityChunk>| {
        chunks
            .into_iter()
            .filter(|chunk| matches!(chunk, CapabilityChunk::Text(_)))
            .count()
    };
    let a = collect_chunks(supervisor.request(&spec, chat("alpha")));
    let b = collect_chunks(supervisor.request(&spec, chat("bravo")));
    let (a, b) = tokio::join!(a, b);

    assert_eq!(count_text(a), 3);
    assert_eq!(count_text(b), 3);
    assert!(supervisor.is_running(&spec.runtime_id));
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn cooperative_cancel_keeps_the_sidecar_warm_and_serves_the_next_request() {
    let spec = spec_with("normal", true, Duration::from_secs(10));
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    {
        let mut stream = supervisor.request(&spec, chat("slow"));
        while let Some(item) = stream.recv().await {
            if let Ok(CapabilityChunk::Text(_)) = item {
                break;
            }
        }
    }

    let mut warm = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        warm = supervisor.is_running(&spec.runtime_id);
        if !warm {
            break;
        }
    }
    assert!(warm, "the sidecar stays warm after a cooperative cancel");

    let mut text = String::new();
    for chunk in collect_chunks(supervisor.request(&spec, chat("again"))).await {
        if let CapabilityChunk::Text(delta) = chunk {
            text.push_str(&delta);
        }
    }
    assert_eq!(text, "again!");
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn a_default_spec_kills_the_sidecar_on_cancel() {
    let spec = spec("normal");
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    {
        let mut stream = supervisor.request(&spec, chat("slow"));
        while let Some(item) = stream.recv().await {
            if let Ok(CapabilityChunk::Text(_)) = item {
                break;
            }
        }
    }

    assert!(dies(&supervisor, &spec.runtime_id).await);
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn cooperative_cancel_without_ack_kills_after_grace() {
    let spec = spec_with("normal", true, Duration::from_millis(400));
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    {
        let mut stream = supervisor.request(&spec, chat("deaf"));
        while let Some(item) = stream.recv().await {
            if let Ok(CapabilityChunk::Status(_)) = item {
                break;
            }
        }
    }
    assert!(
        dies(&supervisor, &spec.runtime_id).await,
        "unacked cancel kills after grace"
    );
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn frame_timeout_kills_a_hanging_sidecar_and_a_successor_starts_clean() {
    let mut spec = spec("hang-after-begin");
    spec.frame_timeout = Duration::from_millis(300);
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    let stream = supervisor.request(&spec, chat("hi"));
    let errored = stream_errors(stream).await;
    assert!(errored, "a hung request fails");
    assert!(dies(&supervisor, &spec.runtime_id).await);

    let mut good = spec.clone();
    good.arguments = vec!["python3".to_owned(), script_path(), "normal".to_owned()];
    good.frame_timeout = Duration::from_secs(600);
    supervisor
        .ensure_running(&good)
        .await
        .expect("respawn ready");
    let mut text = String::new();
    for chunk in collect_chunks(supervisor.request(&good, chat("pong"))).await {
        if let CapabilityChunk::Text(delta) = chunk {
            text.push_str(&delta);
        }
    }
    assert!(text.contains("pong"));
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn a_slow_but_progressing_stream_completes_without_a_false_timeout() {
    let mut spec = spec("normal");
    spec.frame_timeout = Duration::from_millis(400);
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    let tokens = collect_chunks(supervisor.request(&spec, chat("slow")))
        .await
        .into_iter()
        .filter(|chunk| matches!(chunk, CapabilityChunk::Text(_)))
        .count();
    assert_eq!(tokens, 20);
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn a_cancelled_queued_request_never_sends_its_op() {
    let dir = TempDir::new();
    let op_log = dir.join("ops.log");
    let mut spec = spec_with("normal", true, Duration::from_secs(10));
    spec.environment
        .insert("HEDOS_OP_LOG".to_owned(), op_log.display().to_string());
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    let running = supervisor.clone();
    let a_spec = spec.clone();
    let a = tokio::spawn(async move {
        let _ = collect_chunks(running.request(&a_spec, chat("slow"))).await;
    });
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        if std::fs::read_to_string(&op_log)
            .map(|log| log.contains("chat"))
            .unwrap_or(false)
        {
            break;
        }
    }

    let b_stream = supervisor.request(&spec, chat("again"));
    let b = tokio::spawn(async move {
        let mut b_stream = b_stream;
        let _ = b_stream.recv().await;
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    b.abort();

    let _ = a.await;
    let _ = collect_chunks(supervisor.request(&spec, chat("again"))).await;

    let log = std::fs::read_to_string(&op_log).unwrap_or_default();
    let chat_ops = log.lines().filter(|line| *line == "chat").count();
    assert_eq!(chat_ops, 2, "the cancelled queued request sent no op");
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn a_job_cancel_with_ack_keeps_the_sidecar_warm() {
    let spec = spec("normal");
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    {
        let mut stream = supervisor.job_request(
            &spec,
            jobj(&[("op", jstr("image")), ("steps", JsonValue::Int(50))]),
        );
        while let Some(item) = stream.recv().await {
            if let Ok(JobRuntimeEvent::Progress { .. }) = item {
                break;
            }
        }
    }

    let mut warm = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        warm = supervisor.is_running(&spec.runtime_id);
        if !warm {
            break;
        }
    }
    assert!(warm, "an acked job cancel keeps the sidecar warm");

    let mut saw_result = false;
    let mut stream = supervisor.job_request(
        &spec,
        jobj(&[("op", jstr("image")), ("steps", JsonValue::Int(2))]),
    );
    while let Some(item) = stream.recv().await {
        if let Ok(JobRuntimeEvent::Result { .. }) = item {
            saw_result = true;
        }
    }
    assert!(saw_result, "the next job produces a result");
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn a_job_cancel_without_ack_kills_after_grace() {
    let spec = spec_with("normal", false, Duration::from_millis(400));
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    {
        let mut stream = supervisor.job_request(
            &spec,
            jobj(&[("op", jstr("image")), ("prompt", jstr("deaf"))]),
        );
        while let Some(item) = stream.recv().await {
            if let Ok(JobRuntimeEvent::Started) = item {
                break;
            }
        }
    }
    assert!(dies(&supervisor, &spec.runtime_id).await);
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn terminate_all_kills_sidecars_without_graceful_shutdown() {
    let spec = spec("normal");
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");
    assert!(supervisor.process_identifier(&spec.runtime_id).is_some());

    supervisor.terminate_all();
    assert!(!supervisor.is_running(&spec.runtime_id));
}

#[tokio::test]
async fn times_out_when_the_sidecar_never_becomes_ready() {
    let mut spec = spec("never-ready");
    spec.ready_timeout = Duration::from_millis(800);
    let supervisor = SidecarSupervisor::new();
    let result = supervisor.ensure_running(&spec).await;
    assert!(matches!(result, Err(SidecarError::SidecarDied { .. })));
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn restart_after_shutdown_survives_the_old_generations_late_eof() {
    let spec = spec("normal");
    let supervisor = SidecarSupervisor::new();
    for _ in 0..3 {
        supervisor.ensure_running(&spec).await.expect("ready");
        supervisor.shutdown(&spec.runtime_id).await;
        supervisor
            .ensure_running(&spec)
            .await
            .expect("respawn ready");
        let vectors = collect_chunks(supervisor.request(
            &spec,
            jobj(&[("op", jstr("embed")), ("input", jstr("hedos"))]),
        ))
        .await
        .into_iter()
        .filter(|chunk| matches!(chunk, CapabilityChunk::Vector(_)))
        .count();
        assert_eq!(vectors, 1);
        supervisor.shutdown(&spec.runtime_id).await;
    }
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn dropping_the_stream_after_done_leaves_a_default_sidecar_warm() {
    // Regression: the non-cooperative consumer-drop watcher must not kill the
    // sidecar after a normal completion. A default (non-cooperative) spec, a
    // request consumed through `Done` then dropped early, must stay warm.
    let spec = spec("normal");
    let supervisor = SidecarSupervisor::new();
    supervisor.ensure_running(&spec).await.expect("ready");

    {
        let mut stream = supervisor.request(&spec, chat("abcdef"));
        while let Some(item) = stream.recv().await {
            if let Ok(CapabilityChunk::Done(_)) = item {
                break;
            }
        }
    }

    let mut warm = true;
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        warm = supervisor.is_running(&spec.runtime_id);
        if !warm {
            break;
        }
    }
    assert!(warm, "a completed request must not kill the warm sidecar");

    let mut text = String::new();
    for chunk in collect_chunks(supervisor.request(&spec, chat("again"))).await {
        if let CapabilityChunk::Text(delta) = chunk {
            text.push_str(&delta);
        }
    }
    assert_eq!(text, "again!", "the warm sidecar still serves");
    supervisor.shutdown_all().await;
}

#[tokio::test]
async fn concurrent_ensure_running_spawns_a_single_process() {
    let spec = spec("normal");
    let supervisor = SidecarSupervisor::new();

    let a = {
        let supervisor = supervisor.clone();
        let spec = spec.clone();
        tokio::spawn(async move { supervisor.ensure_running(&spec).await })
    };
    let b = {
        let supervisor = supervisor.clone();
        let spec = spec.clone();
        tokio::spawn(async move { supervisor.ensure_running(&spec).await })
    };
    assert!(a.await.expect("join a").is_ok());
    assert!(b.await.expect("join b").is_ok());

    assert!(supervisor.is_running(&spec.runtime_id));
    let pid = supervisor
        .process_identifier(&spec.runtime_id)
        .expect("pid");

    let mut text = String::new();
    for chunk in collect_chunks(supervisor.request(&spec, chat("x"))).await {
        if let CapabilityChunk::Text(delta) = chunk {
            text.push_str(&delta);
        }
    }
    assert_eq!(text, "x!", "the single surviving sidecar serves");
    assert_eq!(
        supervisor.process_identifier(&spec.runtime_id),
        Some(pid),
        "no respawn happened"
    );
    supervisor.shutdown_all().await;
}

async fn stream_errors(mut stream: SidecarStream<CapabilityChunk>) -> bool {
    while let Some(item) = stream.recv().await {
        if item.is_err() {
            return true;
        }
    }
    false
}

async fn dies(supervisor: &SidecarSupervisor, id: &str) -> bool {
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if !supervisor.is_running(id) {
            return true;
        }
    }
    false
}
