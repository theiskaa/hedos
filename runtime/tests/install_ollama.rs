//! Tests for the Ollama install provider, driven by a mock daemon that answers
//! `/api/tags` (reachability) and `/api/pull` (an ndjson progress stream).

use std::collections::HashMap;

use kernel::install::{InstallError, InstallProviderId, InstallStreamEvent};
use runtime::install::{InstallProvider, OllamaInstallProvider};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

struct MockDaemon {
    base_url: String,
    accept: AbortHandle,
}

impl Drop for MockDaemon {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

/// A daemon that returns 200 for `/api/tags` and streams `pull_body` for
/// `/api/pull`. `pull_status` lets a test force a non-200 pull.
async fn daemon(pull_status: &'static str, pull_body: &'static str) -> MockDaemon {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let accept = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let read = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]);
                let (status, body) = if request.contains("/api/pull") {
                    (pull_status, pull_body)
                } else {
                    ("200 OK", "{}")
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(body.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    })
    .abort_handle();
    MockDaemon {
        base_url: format!("http://{addr}"),
        accept,
    }
}

fn provider(base_url: &str) -> OllamaInstallProvider {
    // An empty PATH so `daemon_binary` finds nothing under our control; the mock's
    // reachable `/api/tags` is what makes it available, and pulls skip start_daemon.
    OllamaInstallProvider::with_config(base_url, HashMap::new())
}

async fn collect(mut rx: runtime::install::InstallEventStream) -> Vec<InstallStreamEvent> {
    let mut events = Vec::new();
    while let Some(item) = rx.recv().await {
        events.push(item.expect("no stream error"));
    }
    events
}

#[tokio::test]
async fn a_successful_pull_streams_status_and_progress_then_ends_cleanly() {
    let body = concat!(
        "{\"status\":\"pulling manifest\"}\n",
        "{\"status\":\"downloading\",\"digest\":\"sha256:a\",\"total\":100,\"completed\":40}\n",
        "{\"status\":\"downloading\",\"digest\":\"sha256:a\",\"total\":100,\"completed\":100}\n",
        "{\"status\":\"success\"}\n",
    );
    let server = daemon("200 OK", body).await;
    let plan = provider(&server.base_url)
        .plan("gemma3:4b")
        .await
        .expect("plan");
    assert_eq!(plan.reference, "gemma3:4b");
    assert_eq!(plan.provider, InstallProviderId::ollama());

    let events = collect(provider(&server.base_url).install(plan)).await;
    // A status line for the manifest, then two aggregated progress events.
    assert!(events
        .iter()
        .any(|event| matches!(event, InstallStreamEvent::Status(message) if message == "pulling manifest")));
    let progress: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            InstallStreamEvent::Progress(progress) => Some(progress.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(progress.len(), 2);
    assert_eq!(progress[0].bytes_downloaded, 40);
    assert_eq!(progress[1].bytes_downloaded, 100);
    assert_eq!(progress[1].total_bytes, Some(100));
    // Ending cleanly (the loop above saw `None`) means success was reported.
}

#[tokio::test]
async fn a_success_line_without_a_trailing_newline_still_completes() {
    // The server ends the stream with no `\n` after the success line — it must
    // still be folded (matching Swift's trailing-buffer flush at EOF).
    let body = concat!(
        "{\"status\":\"pulling manifest\"}\n",
        "{\"status\":\"success\"}",
    );
    let server = daemon("200 OK", body).await;
    let plan = provider(&server.base_url).plan("m").await.expect("plan");
    let mut rx = provider(&server.base_url).install(plan);
    let mut error = None;
    while let Some(item) = rx.recv().await {
        if let Err(e) = item {
            error = Some(e);
        }
    }
    assert!(
        error.is_none(),
        "unterminated success line should not error"
    );
}

#[tokio::test]
async fn a_pull_that_never_reports_success_fails() {
    // Well-formed lines but no `success` — the stream ends and we surface an error.
    let server = daemon("200 OK", "{\"status\":\"downloading\"}\n").await;
    let plan = provider(&server.base_url)
        .plan("mymodel")
        .await
        .expect("plan");
    let mut rx = provider(&server.base_url).install(plan);
    let mut last = None;
    while let Some(item) = rx.recv().await {
        last = Some(item);
    }
    match last {
        Some(Err(InstallError::TransferFailed(message))) => {
            assert!(message.contains("without reporting success"), "{message}")
        }
        other => panic!("expected a transfer failure, got {other:?}"),
    }
}

#[tokio::test]
async fn an_error_line_in_the_stream_fails_the_pull() {
    let server = daemon("200 OK", "{\"error\":\"pull access denied\"}\n").await;
    let plan = provider(&server.base_url)
        .plan("gated")
        .await
        .expect("plan");
    let mut rx = provider(&server.base_url).install(plan);
    let mut last = None;
    while let Some(item) = rx.recv().await {
        last = Some(item);
    }
    match last {
        Some(Err(InstallError::TransferFailed(message))) => {
            assert!(message.contains("pull access denied"), "{message}")
        }
        other => panic!("expected a transfer failure, got {other:?}"),
    }
}

#[tokio::test]
async fn a_non_200_pull_surfaces_the_body_error() {
    let server = daemon("500 Internal Server Error", "{\"error\":\"boom\"}").await;
    let plan = provider(&server.base_url).plan("m").await.expect("plan");
    let mut rx = provider(&server.base_url).install(plan);
    let mut last = None;
    while let Some(item) = rx.recv().await {
        last = Some(item);
    }
    match last {
        Some(Err(InstallError::TransferFailed(message))) => {
            assert_eq!(message, "ollama: boom")
        }
        other => panic!("expected a transfer failure, got {other:?}"),
    }
}

#[tokio::test]
async fn search_is_unsupported() {
    let provider = OllamaInstallProvider::with_config("http://127.0.0.1:1", HashMap::new());
    match provider.search("anything", 5).await {
        Err(InstallError::ProviderUnavailable(message)) => assert!(message.contains("no search")),
        other => panic!("expected provider-unavailable, got {other:?}"),
    }
}

#[tokio::test]
async fn an_invalid_reference_is_rejected_by_plan() {
    let provider = OllamaInstallProvider::with_config("http://127.0.0.1:1", HashMap::new());
    // A leading slash isn't a valid tag shape.
    match provider.plan("///").await {
        Err(InstallError::ReferenceInvalid(_)) => {}
        other => panic!("expected reference-invalid, got {other:?}"),
    }
}

#[tokio::test]
async fn availability_is_ready_when_the_daemon_answers() {
    let server = daemon("200 OK", "{}").await;
    let provider = provider(&server.base_url);
    assert_eq!(
        provider.availability().await,
        kernel::install::InstallAvailability::Ready
    );
}
