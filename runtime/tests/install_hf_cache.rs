//! Tests for the Hugging Face cache writer, driven against temp dirs and a mock
//! transport that understands range requests (for resume/416 paths).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kernel::install::InstallError;
use kernel::install::file_selection::HFSibling;
use runtime::install::transport::{StreamFuture, StreamStart, TransportFuture};
use runtime::install::{HFCacheLayout, HFCacheWriter, InstallRequest, InstallTransport};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

fn sha_hex(data: &[u8]) -> String {
    let mut out = String::new();
    for byte in Sha256::digest(data) {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// A transport that serves `body`, honoring `Range: bytes=N-` with a 206, and
/// optionally forcing a status (404/403/…) or a 416 on ranged requests.
struct StreamMock {
    body: Vec<u8>,
    force_status: Option<u16>,
    range_416: bool,
    // Answer a ranged request with a full 200 (server ignored the Range).
    ignore_range: bool,
}

impl StreamMock {
    fn serving(body: &[u8]) -> Arc<Self> {
        Arc::new(Self {
            body: body.to_vec(),
            force_status: None,
            range_416: false,
            ignore_range: false,
        })
    }
}

impl InstallTransport for StreamMock {
    fn fetch(&self, _request: InstallRequest) -> TransportFuture {
        Box::pin(async { Err(InstallError::TransferFailed("unused".to_owned())) })
    }

    fn stream(&self, request: InstallRequest) -> StreamFuture {
        let range = request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("range"))
            .and_then(|(_, value)| value.strip_prefix("bytes="))
            .and_then(|value| value.trim_end_matches('-').parse::<usize>().ok());
        let force_status = self.force_status;
        let range_416 = self.range_416;
        let ignore_range = self.ignore_range;
        let body = self.body.clone();
        Box::pin(async move {
            let (status, payload): (u16, Vec<u8>) = match (force_status, range) {
                (Some(status), _) => (status, Vec::new()),
                (None, Some(_start)) if range_416 => (416, Vec::new()),
                // Server ignored the Range — send the whole body with a 200.
                (None, Some(_start)) if ignore_range => (200, body),
                (None, Some(start)) => (206, body.get(start..).unwrap_or(&[]).to_vec()),
                (None, None) => (200, body),
            };
            let (tx, chunks) = mpsc::channel(64);
            tokio::spawn(async move {
                // Two chunks, to exercise multi-chunk reassembly.
                let mid = payload.len() / 2;
                for slice in [&payload[..mid], &payload[mid..]] {
                    if !slice.is_empty() {
                        let _ = tx.send(Ok(slice.to_vec())).await;
                    }
                }
            });
            Ok(StreamStart { status, chunks })
        })
    }
}

fn temp_root() -> PathBuf {
    // A unique-enough dir without pulling in a temp-dir crate (no Date/rand in libs,
    // but tests may use them): thread id + a nanosecond stamp.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "hedos-hfcache-{stamp}-{:?}",
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn writer(root: &Path, transport: Arc<dyn InstallTransport>) -> HFCacheWriter {
    HFCacheWriter::new(HFCacheLayout::new(root, "org/Model"), transport)
}

async fn download_one(
    writer: &HFCacheWriter,
    sibling: &HFSibling,
    revision: &str,
) -> Result<i64, InstallError> {
    let mut total = 0i64;
    writer
        .download(
            sibling,
            revision,
            InstallRequest::get("http://hf.test/file"),
            &mut |delta| total += delta,
        )
        .await?;
    Ok(total)
}

#[tokio::test]
async fn a_full_download_lands_as_a_content_addressed_blob_with_a_snapshot_symlink() {
    let root = temp_root();
    let body = b"the model weights, such as they are".to_vec();
    let sha = sha_hex(&body);
    let sibling =
        HFSibling::new("model.safetensors", Some(body.len() as i64)).with_sha256(Some(sha.clone()));
    let writer = writer(&root, StreamMock::serving(&body));
    writer.prepare_skeleton("rev1", None).expect("skeleton");

    let counted = download_one(&writer, &sibling, "rev1")
        .await
        .expect("download");
    assert_eq!(counted, body.len() as i64);

    let layout = writer.layout();
    let blob = layout.repo_directory().join("blobs").join(&sha);
    assert!(blob.exists(), "blob missing");
    assert_eq!(std::fs::read(&blob).unwrap(), body);

    let snapshot = layout
        .repo_directory()
        .join("snapshots")
        .join("rev1")
        .join("model.safetensors");
    // The snapshot is a relative symlink into blobs/ that resolves to the content.
    let target = std::fs::read_link(&snapshot).expect("symlink");
    assert_eq!(target, PathBuf::from(format!("../../blobs/{sha}")));
    assert_eq!(std::fs::read(&snapshot).unwrap(), body);

    writer.commit_ref("rev1").expect("commit");
    let reference = layout.repo_directory().join("refs").join("main");
    assert_eq!(std::fs::read_to_string(&reference).unwrap(), "rev1");

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn a_plain_file_without_an_lfs_hash_is_named_by_its_digest() {
    let root = temp_root();
    let body = b"config contents".to_vec();
    let sibling = HFSibling::new("config.json", Some(body.len() as i64));
    let writer = writer(&root, StreamMock::serving(&body));
    writer.prepare_skeleton("rev1", None).expect("skeleton");
    download_one(&writer, &sibling, "rev1")
        .await
        .expect("download");

    let blob = writer
        .layout()
        .repo_directory()
        .join("blobs")
        .join(sha_hex(&body));
    assert!(blob.exists(), "digest-named blob missing");
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn an_existing_blob_is_reused_without_downloading() {
    let root = temp_root();
    let body = b"already here".to_vec();
    let sha = sha_hex(&body);
    let sibling =
        HFSibling::new("model.bin", Some(body.len() as i64)).with_sha256(Some(sha.clone()));
    let writer = writer(&root, StreamMock::serving(b"DIFFERENT - must not be read"));
    writer.prepare_skeleton("rev1", None).expect("skeleton");
    // Pre-place the content-addressed blob.
    let blob = writer.layout().repo_directory().join("blobs").join(&sha);
    std::fs::write(&blob, &body).unwrap();

    let counted = download_one(&writer, &sibling, "rev1")
        .await
        .expect("download");
    // Counted the size without touching the (wrong) transport body.
    assert_eq!(counted, body.len() as i64);
    let snapshot = writer
        .layout()
        .repo_directory()
        .join("snapshots/rev1/model.bin");
    assert_eq!(std::fs::read(&snapshot).unwrap(), body);
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn a_saved_partial_resumes_via_a_range_request() {
    let root = temp_root();
    let body = b"0123456789abcdefghij".to_vec();
    let sha = sha_hex(&body);
    let sibling =
        HFSibling::new("model.safetensors", Some(body.len() as i64)).with_sha256(Some(sha.clone()));
    let writer = writer(&root, StreamMock::serving(&body));
    writer.prepare_skeleton("rev1", None).expect("skeleton");
    // Save the first 8 bytes as an in-progress incomplete under the sha name.
    let incomplete = writer
        .layout()
        .repo_directory()
        .join("blobs")
        .join(format!("{sha}.incomplete"));
    std::fs::write(&incomplete, &body[..8]).unwrap();

    let counted = download_one(&writer, &sibling, "rev1")
        .await
        .expect("download");
    // 8 pre-existing + 12 streamed = full length.
    assert_eq!(counted, body.len() as i64);
    let blob = writer.layout().repo_directory().join("blobs").join(&sha);
    assert_eq!(std::fs::read(&blob).unwrap(), body);
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn a_range_not_satisfiable_restarts_the_download() {
    let root = temp_root();
    let body = b"restart from zero".to_vec();
    let sha = sha_hex(&body);
    let sibling =
        HFSibling::new("model.bin", Some(body.len() as i64)).with_sha256(Some(sha.clone()));
    let transport = Arc::new(StreamMock {
        body: body.clone(),
        force_status: None,
        range_416: true,
        ignore_range: false,
    });
    let writer = writer(&root, transport);
    writer.prepare_skeleton("rev1", None).expect("skeleton");
    // A stale partial that's now past the (unchanged) end → server says 416.
    let incomplete = writer
        .layout()
        .repo_directory()
        .join("blobs")
        .join(format!("{sha}.incomplete"));
    std::fs::write(&incomplete, b"stale bytes that are wrong").unwrap();

    let counted = download_one(&writer, &sibling, "rev1")
        .await
        .expect("download");
    // Net accounting through the discard+restart: exactly the file size.
    assert_eq!(counted, body.len() as i64);
    let blob = writer.layout().repo_directory().join("blobs").join(&sha);
    assert_eq!(std::fs::read(&blob).unwrap(), body);
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn a_server_that_ignores_the_range_discards_the_partial_and_restarts() {
    let root = temp_root();
    let body = b"full body wins over the stale partial".to_vec();
    let sha = sha_hex(&body);
    let sibling =
        HFSibling::new("model.bin", Some(body.len() as i64)).with_sha256(Some(sha.clone()));
    let transport = Arc::new(StreamMock {
        body: body.clone(),
        force_status: None,
        range_416: false,
        ignore_range: true,
    });
    let writer = writer(&root, transport);
    writer.prepare_skeleton("rev1", None).expect("skeleton");
    let incomplete = writer
        .layout()
        .repo_directory()
        .join("blobs")
        .join(format!("{sha}.incomplete"));
    std::fs::write(&incomplete, b"stale head").unwrap();

    let counted = download_one(&writer, &sibling, "rev1")
        .await
        .expect("download");
    // +stale, -stale (discarded on the 200), +full = exactly the file size.
    assert_eq!(counted, body.len() as i64);
    let blob = writer.layout().repo_directory().join("blobs").join(&sha);
    assert_eq!(std::fs::read(&blob).unwrap(), body);
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn a_checksum_mismatch_is_reported_and_the_partial_removed() {
    let root = temp_root();
    let body = b"real bytes".to_vec();
    let sibling = HFSibling::new("model.bin", Some(body.len() as i64))
        .with_sha256(Some("00deadbeef".to_owned()));
    let writer = writer(&root, StreamMock::serving(&body));
    writer.prepare_skeleton("rev1", None).expect("skeleton");
    match download_one(&writer, &sibling, "rev1").await {
        Err(InstallError::ChecksumMismatch(file)) => assert_eq!(file, "model.bin"),
        other => panic!("expected checksum mismatch, got {other:?}"),
    }
    let incomplete = writer
        .layout()
        .repo_directory()
        .join("blobs")
        .join("00deadbeef.incomplete");
    assert!(
        !incomplete.exists(),
        "partial should be removed on mismatch"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn a_short_download_is_a_size_mismatch() {
    let root = temp_root();
    let body = b"only ten!!".to_vec();
    let sibling = HFSibling::new("model.bin", Some(body.len() as i64 + 5));
    let writer = writer(&root, StreamMock::serving(&body));
    writer.prepare_skeleton("rev1", None).expect("skeleton");
    match download_one(&writer, &sibling, "rev1").await {
        Err(InstallError::TransferFailed(message)) => assert!(message.contains("ended after")),
        other => panic!("expected size mismatch, got {other:?}"),
    }
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn http_errors_map_to_auth_and_missing() {
    for (status, check) in [(403u16, "auth"), (404u16, "missing"), (500u16, "other")] {
        let root = temp_root();
        let sibling = HFSibling::new("model.bin", Some(4));
        let transport = Arc::new(StreamMock {
            body: Vec::new(),
            force_status: Some(status),
            range_416: false,
            ignore_range: false,
        });
        let writer = writer(&root, transport);
        writer.prepare_skeleton("rev1", None).expect("skeleton");
        let result = download_one(&writer, &sibling, "rev1").await;
        match (check, result) {
            ("auth", Err(InstallError::AuthRequired(_))) => {}
            ("missing", Err(InstallError::TransferFailed(m))) if m.contains("missing") => {}
            ("other", Err(InstallError::TransferFailed(m))) if m.contains("HTTP 500") => {}
            other => panic!("unexpected for {status}: {other:?}"),
        }
        std::fs::remove_dir_all(&root).ok();
    }
}

#[tokio::test]
async fn prepare_skeleton_writes_the_ref_and_a_first_weight_placeholder() {
    let root = temp_root();
    let writer = writer(&root, StreamMock::serving(b""));
    writer
        .prepare_skeleton("revX", Some("weightblob"))
        .expect("skeleton");
    let repo = writer.layout().repo_directory();
    assert_eq!(
        std::fs::read_to_string(repo.join("refs/main")).unwrap(),
        "revX"
    );
    assert!(repo.join("snapshots/revX").is_dir());
    assert!(repo.join("blobs/weightblob.incomplete").exists());
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn interruption_recovery_helpers_behave() {
    let root = temp_root();
    let writer = writer(&root, StreamMock::serving(b""));
    writer.prepare_skeleton("revX", None).expect("skeleton");
    let blobs = writer.layout().repo_directory().join("blobs");

    // A big completed blob → substantial progress + a completed blob present.
    std::fs::write(blobs.join("done"), vec![7u8; 32]).unwrap();
    std::fs::write(blobs.join("part.incomplete"), vec![1u8; 8]).unwrap();
    assert!(writer.has_substantial_progress(16));
    assert!(!writer.has_substantial_progress(1024));
    assert!(writer.has_completed_blob());

    // A fresh stray incomplete is kept; one named in `keeping` is kept too.
    let mut keeping = HashSet::new();
    keeping.insert("part".to_owned());
    writer.remove_stray_incompletes(&keeping).expect("reap");
    assert!(blobs.join("part.incomplete").exists());

    // retreat drops snapshots + refs but keeps blobs.
    writer.retreat_to_blobs_only();
    assert!(!writer.layout().repo_directory().join("refs").exists());
    assert!(!writer.layout().repo_directory().join("snapshots").exists());
    assert!(blobs.join("done").exists());

    writer.remove_repo();
    assert!(!writer.layout().repo_directory().exists());
    std::fs::remove_dir_all(&root).ok();
}
