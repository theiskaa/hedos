//! Tests for the model remover, driven by a mock Ollama daemon and a recording
//! trasher.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kernel::records::{Modality, ModelRecord, ModelSource, ModelState, SourceKind};
use kernel::removal::RemovalError;
use runtime::removal::{ModelRemover, OllamaModelRemover, Trasher};
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

/// 200 for `/api/tags`; `delete_status`/`delete_body` for `/api/delete`.
async fn daemon(delete_status: &'static str, delete_body: &'static str) -> MockDaemon {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let accept = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let read = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]);
                let (status, body) = if request.contains("/api/delete") {
                    (delete_status, delete_body)
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

fn record(kind: SourceKind, path: &str) -> ModelRecord {
    ModelRecord::new(
        "gemma3:4b",
        Modality::text(),
        Vec::new(),
        ModelSource::new(kind, path),
    )
}

fn recording_trasher() -> (Trasher, Arc<Mutex<Vec<PathBuf>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);
    let trasher: Trasher = Box::new(move |path| {
        sink.lock().unwrap().push(path.to_path_buf());
        Ok(())
    });
    (trasher, log)
}

fn ollama(base_url: &str) -> OllamaModelRemover {
    // Force "no binary" so a test never spawns a real `ollama serve`; the mock's
    // reachable `/api/tags` is what lets a delete proceed.
    OllamaModelRemover::with_config(base_url, HashMap::new()).with_binary_present(|| false)
}

#[tokio::test]
async fn an_ollama_model_deletes_through_the_daemon() {
    let server = daemon("200 OK", "{}").await;
    let (trasher, log) = recording_trasher();
    let remover = ModelRemover::new(trasher, ollama(&server.base_url));
    let mut rec = record(SourceKind::ollama(), "");
    rec.footprint_mb = Some(2048);

    let report = remover.remove(&rec).await.expect("remove");
    assert!(report.daemon_deleted);
    assert!(report.trashed_paths.is_empty());
    assert_eq!(report.freed_bytes_estimate, 2048i64 << 20);
    // No files were trashed for a daemon delete.
    assert!(log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_daemon_404_counts_as_deleted() {
    let server = daemon("404 Not Found", "{}").await;
    let remover = ModelRemover::new(recording_trasher().0, ollama(&server.base_url));
    let rec = record(SourceKind::ollama(), "");
    assert!(remover.remove(&rec).await.expect("remove").daemon_deleted);
}

#[tokio::test]
async fn a_daemon_error_surfaces_the_message() {
    let server = daemon("500 Internal Server Error", r#"{"error":"boom"}"#).await;
    let remover = ModelRemover::new(recording_trasher().0, ollama(&server.base_url));
    let rec = record(SourceKind::ollama(), "");
    match remover.remove(&rec).await {
        Err(RemovalError::DaemonDeleteFailed(message)) => assert_eq!(message, "ollama: boom"),
        other => panic!("expected daemon delete failure, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unreachable_daemon_without_a_binary_is_unavailable() {
    // Point at a closed port; empty env → no binary discovered.
    let remover = ModelRemover::new(recording_trasher().0, ollama("http://127.0.0.1:1"));
    let rec = record(SourceKind::ollama(), "");
    match remover.remove(&rec).await {
        Err(RemovalError::DaemonUnavailable(hint)) => {
            assert!(hint.contains("Ollama isn't running"))
        }
        other => panic!("expected daemon unavailable, got {other:?}"),
    }
}

#[tokio::test]
async fn a_folder_model_is_trashed_by_path() {
    let dir = std::env::temp_dir().join(format!(
        "hedos-removal-test-{:?}",
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let (trasher, log) = recording_trasher();
    // The ollama remover is unused for a folder model.
    let remover = ModelRemover::new(trasher, ollama("http://127.0.0.1:1"));
    let mut rec = record(SourceKind::folder(), dir.to_str().unwrap());
    rec.footprint_mb = Some(100);

    let report = remover.remove(&rec).await.expect("remove");
    assert!(!report.daemon_deleted);
    assert_eq!(
        report.trashed_paths,
        vec![dir.to_string_lossy().into_owned()]
    );
    assert_eq!(log.lock().unwrap().as_slice(), std::slice::from_ref(&dir));
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_trasher_failure_becomes_a_trash_failed_error() {
    let dir = std::env::temp_dir().join(format!(
        "hedos-removal-fail-{:?}",
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let trasher: Trasher = Box::new(|_| Err("permission denied".to_owned()));
    let remover = ModelRemover::new(trasher, ollama("http://127.0.0.1:1"));
    let rec = record(SourceKind::folder(), dir.to_str().unwrap());
    match remover.remove(&rec).await {
        Err(RemovalError::TrashFailed { reason, .. }) => assert_eq!(reason, "permission denied"),
        other => panic!("expected trash failure, got {other:?}"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_missing_ollama_model_trashes_nothing_and_skips_the_daemon() {
    // A missing ollama model is not via_daemon (nothing to delete on the daemon)
    // and has no paths — remove is a clean no-op report.
    let (trasher, log) = recording_trasher();
    let remover = ModelRemover::new(trasher, ollama("http://127.0.0.1:1"));
    let mut rec = record(SourceKind::ollama(), "");
    rec.state = ModelState::Missing;
    let report = remover.remove(&rec).await.expect("remove");
    assert!(!report.daemon_deleted);
    assert!(report.trashed_paths.is_empty());
    assert!(log.lock().unwrap().is_empty());
}
