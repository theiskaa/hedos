//! Tests for the model remover, driven by a mock Ollama daemon and a recording
//! trasher.

mod support;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kernel::records::{Modality, ModelRecord, ModelSource, ModelState, SourceKind};
use kernel::removal::RemovalError;
use runtime::removal::{ModelRemover, OllamaModelRemover, Trasher};
use support::TempDir;
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
    rec.footprint_bytes = Some(2048 * (1 << 20));

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
    rec.footprint_bytes = Some(100 * (1 << 20));

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

/// An Ollama store under `root` with two tags sharing a weight blob and a
/// license blob, each with a params blob of its own. Returns the manifests.
fn two_tags_sharing_weights(root: &std::path::Path) -> (PathBuf, PathBuf) {
    let blob = |digit: char, bytes: usize| {
        let digest = format!("sha256:{}", digit.to_string().repeat(64));
        std::fs::create_dir_all(root.join("blobs")).unwrap();
        std::fs::write(
            root.join("blobs").join(digest.replace(':', "-")),
            vec![0u8; bytes],
        )
        .unwrap();
        digest
    };
    let (weights, license, params_a, params_b) =
        (blob('a', 4096), blob('b', 8), blob('c', 16), blob('d', 32));
    let manifest = |tag: &str, params: &str| {
        let path = root.join(format!("manifests/registry.ollama.ai/library/qwen/{tag}"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let layers = [&weights, &license, &params.to_owned()]
            .iter()
            .map(|digest| format!(r#"{{"mediaType":"x","digest":"{digest}","size":1}}"#))
            .collect::<Vec<_>>()
            .join(",");
        std::fs::write(&path, format!(r#"{{"layers":[{layers}]}}"#)).unwrap();
        path
    };
    (
        manifest("27b", &params_a),
        manifest("27b-coding", &params_b),
    )
}

#[tokio::test]
async fn without_ollama_a_model_is_deleted_by_the_files_no_other_tag_uses() {
    let store = TempDir::new();
    let (coding, plain) = two_tags_sharing_weights(store.path());
    let (trasher, log) = recording_trasher();
    let remover = ModelRemover::new(trasher, ollama("http://127.0.0.1:1"));
    let rec = record(SourceKind::ollama(), coding.to_str().unwrap());

    let preview = remover.preview(&rec);
    assert!(!preview.via_daemon);
    assert_eq!(preview.blocked, None);
    let params = store
        .path()
        .join(format!("blobs/sha256-{}", "c".repeat(64)));
    // Blobs first and the manifest last, so a delete that stops partway can
    // be finished from the manifest.
    assert_eq!(
        preview.paths,
        [
            params.to_string_lossy().into_owned(),
            coding.to_string_lossy().into_owned()
        ]
    );
    assert_eq!(
        preview.bytes_estimate,
        16 + std::fs::metadata(&coding).unwrap().len() as i64
    );

    let report = remover.remove(&rec).await.expect("remove");
    assert!(!report.daemon_deleted);
    assert_eq!(log.lock().unwrap().as_slice(), [params, coding.clone()]);
    // The other tag still names the weights and the license, so both stay.
    assert!(plain.exists());
}

/// Remove `manifest`'s model with Ollama not here, expecting a refusal that
/// deletes nothing.
async fn refused(manifest: &std::path::Path) {
    let (trasher, log) = recording_trasher();
    let remover = ModelRemover::new(trasher, ollama("http://127.0.0.1:1"));
    let rec = record(SourceKind::ollama(), manifest.to_str().unwrap());
    assert!(remover.preview(&rec).blocked.is_some());
    match remover.remove(&rec).await {
        Err(RemovalError::DaemonUnavailable(why)) => {
            assert!(
                why.starts_with("Ollama isn't installed to delete ")
                    && why.ends_with(", so hedos deletes nothing"),
                "{why}"
            )
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn without_ollama_a_digest_that_leaves_the_store_deletes_nothing() {
    let store = TempDir::new();
    let (coding, _) = two_tags_sharing_weights(store.path());
    let outside = store.path().join("outside.txt");
    std::fs::write(&outside, b"keep me").unwrap();
    std::fs::write(
        &coding,
        r#"{"layers":[{"mediaType":"x","digest":"../outside.txt","size":1}]}"#,
    )
    .unwrap();
    refused(&coding).await;
    assert!(outside.exists());
}

#[tokio::test]
async fn without_ollama_a_symlink_among_the_manifests_deletes_nothing() {
    let store = TempDir::new();
    let (coding, plain) = two_tags_sharing_weights(store.path());
    // A tag kept as a link may name the same weights.
    let linked = plain.with_file_name("linked");
    std::os::unix::fs::symlink(&plain, &linked).unwrap();
    refused(&coding).await;
}

#[tokio::test]
async fn without_ollama_a_manifest_path_through_dot_dot_deletes_nothing() {
    let store = TempDir::new();
    let (coding, _) = two_tags_sharing_weights(store.path());
    let dotted = coding.parent().unwrap().join("..").join("qwen").join("27b");
    refused(&dotted).await;
}

#[tokio::test]
async fn without_ollama_finders_files_in_the_store_are_passed_over() {
    let store = TempDir::new();
    let (coding, _) = two_tags_sharing_weights(store.path());
    std::fs::write(coding.with_file_name(".DS_Store"), b"\0\0\0\x01Bud1").unwrap();
    let remover = ModelRemover::new(recording_trasher().0, ollama("http://127.0.0.1:1"));
    let rec = record(SourceKind::ollama(), coding.to_str().unwrap());
    assert_eq!(remover.preview(&rec).blocked, None);
}

#[tokio::test]
async fn without_ollama_a_blob_another_hard_link_keeps_frees_nothing() {
    let store = TempDir::new();
    let (coding, _) = two_tags_sharing_weights(store.path());
    let params = store
        .path()
        .join(format!("blobs/sha256-{}", "c".repeat(64)));
    std::fs::hard_link(&params, store.path().join("kept")).unwrap();
    let remover = ModelRemover::new(recording_trasher().0, ollama("http://127.0.0.1:1"));
    let rec = record(SourceKind::ollama(), coding.to_str().unwrap());
    assert_eq!(
        remover.preview(&rec).bytes_estimate,
        std::fs::metadata(&coding).unwrap().len() as i64
    );
}

#[tokio::test]
async fn without_ollama_an_unreadable_manifest_beside_it_keeps_the_model() {
    // Another tag's manifest that cannot be read may name these blobs too.
    let store = TempDir::new();
    let (coding, _) = two_tags_sharing_weights(store.path());
    let broken = store
        .path()
        .join("manifests/registry.ollama.ai/library/other/latest");
    std::fs::create_dir_all(broken.parent().unwrap()).unwrap();
    std::fs::write(&broken, b"not json").unwrap();
    let (trasher, log) = recording_trasher();
    let remover = ModelRemover::new(trasher, ollama("http://127.0.0.1:1"));
    let rec = record(SourceKind::ollama(), coding.to_str().unwrap());
    assert!(matches!(
        remover.remove(&rec).await,
        Err(RemovalError::DaemonUnavailable(_))
    ));
    assert!(log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn without_ollama_a_digest_in_capitals_or_missing_deletes_nothing() {
    for layers in [
        format!(
            r#"[{{"mediaType":"x","digest":"sha256:{}","size":1}}]"#,
            "C".repeat(64)
        ),
        r#"[{"mediaType":"x","size":1}]"#.to_owned(),
    ] {
        let store = TempDir::new();
        let (coding, _) = two_tags_sharing_weights(store.path());
        std::fs::write(&coding, format!(r#"{{"layers":{layers}}}"#)).unwrap();
        refused(&coding).await;
    }
}

#[tokio::test]
async fn another_tags_digest_in_capitals_still_keeps_the_blob() {
    // A disk that ignores case reads both spellings as one blob.
    let store = TempDir::new();
    let (coding, plain) = two_tags_sharing_weights(store.path());
    let params = format!("sha256:{}", "c".repeat(64));
    let shouted = std::fs::read_to_string(&plain).unwrap().replace(
        &format!("sha256:{}", "d".repeat(64)),
        &params.to_uppercase(),
    );
    std::fs::write(&plain, shouted).unwrap();
    let remover = ModelRemover::new(recording_trasher().0, ollama("http://127.0.0.1:1"));
    let rec = record(SourceKind::ollama(), coding.to_str().unwrap());
    assert_eq!(
        remover.preview(&rec).paths,
        [coding.to_string_lossy().into_owned()]
    );
}

#[tokio::test]
async fn while_ollama_is_here_a_missing_model_has_none_of_its_files_deleted() {
    let store = TempDir::new();
    let (coding, _) = two_tags_sharing_weights(store.path());
    let present = OllamaModelRemover::with_config("http://127.0.0.1:1", HashMap::new())
        .with_binary_present(|| true);
    let (trasher, log) = recording_trasher();
    let remover = ModelRemover::new(trasher, present);
    let mut rec = record(SourceKind::ollama(), coding.to_str().unwrap());
    rec.state = ModelState::Missing;
    assert!(remover.preview(&rec).paths.is_empty());
    let report = remover.remove(&rec).await.expect("remove");
    assert!(report.trashed_paths.is_empty());
    assert!(log.lock().unwrap().is_empty());
}
