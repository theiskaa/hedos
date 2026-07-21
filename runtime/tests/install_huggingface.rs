//! End-to-end tests for the Hugging Face install provider, driven by a mock
//! transport that answers both the hub API (`fetch`) and file downloads
//! (`stream`), writing into a temp hub-cache root.

use std::path::PathBuf;
use std::sync::Arc;

use kernel::install::{InstallError, InstallProviderId, InstallStreamEvent};
use runtime::install::transport::{StreamFuture, StreamStart, TransportFuture};
use runtime::install::{
    HFHubAPI, HuggingFaceInstallProvider, InstallProvider, InstallRequest, InstallResponse,
    InstallTransport,
};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

fn sha_hex(data: &[u8]) -> String {
    let mut out = String::new();
    for byte in Sha256::digest(data) {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Serves a fixed repo: `fetch` returns model-info/search JSON, `stream` returns
/// the one weight file's bytes.
struct HubMock {
    model_info: String,
    search: String,
    file_body: Vec<u8>,
}

impl InstallTransport for HubMock {
    fn fetch(&self, request: InstallRequest) -> TransportFuture {
        let body = if request.url.contains("/api/models/") {
            self.model_info.clone()
        } else {
            self.search.clone()
        };
        Box::pin(async move {
            Ok(InstallResponse {
                status: 200,
                body: body.into_bytes(),
            })
        })
    }

    fn stream(&self, request: InstallRequest) -> StreamFuture {
        let range = request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("range"))
            .and_then(|(_, value)| value.strip_prefix("bytes="))
            .and_then(|value| value.trim_end_matches('-').parse::<usize>().ok());
        let body = self.file_body.clone();
        Box::pin(async move {
            let (status, payload) = match range {
                Some(start) => (206u16, body.get(start..).unwrap_or(&[]).to_vec()),
                None => (200u16, body),
            };
            let (tx, chunks) = mpsc::channel(64);
            tokio::spawn(async move {
                if !payload.is_empty() {
                    let _ = tx.send(Ok(payload)).await;
                }
            });
            Ok(StreamStart { status, chunks })
        })
    }
}

fn temp_root() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "hedos-hfprov-{stamp}-{:?}",
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn provider(root: &PathBuf, body: &[u8]) -> HuggingFaceInstallProvider {
    let sha = sha_hex(body);
    let siblings = format!(
        r#"{{"rfilename":"model.Q4_K_M.gguf","size":{size},"lfs":{{"size":{size},"oid":"{sha}"}}}}"#,
        size = body.len(),
    );
    provider_custom(root, body, false, &siblings, None)
}

/// A provider over a repo with the given `siblings` JSON, gated flag, and token.
fn provider_custom(
    root: &PathBuf,
    body: &[u8],
    gated: bool,
    siblings: &str,
    token: Option<&str>,
) -> HuggingFaceInstallProvider {
    let model_info =
        format!(r#"{{"id":"org/Model","sha":"rev1","gated":{gated},"siblings":[{siblings}]}}"#,);
    let search = r#"[{"id":"org/Model","downloads":5,"likes":1}]"#.to_owned();
    let transport: Arc<dyn InstallTransport> = Arc::new(HubMock {
        model_info,
        search,
        file_body: body.to_vec(),
    });
    let api = HFHubAPI::new(Arc::clone(&transport))
        .with_base_url("https://hf.test")
        .with_token(token.map(str::to_owned));
    HuggingFaceInstallProvider::new(api, transport, root, "/home/me")
}

async fn drain(
    mut rx: runtime::install::InstallEventStream,
) -> Result<Vec<InstallStreamEvent>, InstallError> {
    let mut events = Vec::new();
    while let Some(item) = rx.recv().await {
        events.push(item?);
    }
    Ok(events)
}

#[tokio::test]
async fn plan_resolves_files_revision_and_totals() {
    let root = temp_root();
    let body = vec![9u8; 4096];
    let provider = provider(&root, &body);
    let plan = provider.plan("org/Model").await.expect("plan");
    assert_eq!(plan.provider, InstallProviderId::huggingface());
    assert_eq!(plan.reference, "org/Model");
    assert_eq!(plan.display_name, "Model");
    assert_eq!(plan.revision.as_deref(), Some("rev1"));
    assert_eq!(plan.total_bytes, Some(body.len() as i64));
    assert_eq!(plan.remaining_bytes, Some(body.len() as i64)); // nothing on disk yet
    assert!(!plan.requires_auth);
    assert_eq!(plan.files.len(), 1);
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn install_downloads_into_the_hub_cache_and_streams_progress() {
    let root = temp_root();
    let body = vec![3u8; 8192];
    let sha = sha_hex(&body);
    let provider = provider(&root, &body);
    let plan = provider.plan("org/Model").await.expect("plan");
    let events = drain(provider.install(plan)).await.expect("install");

    // A resolving status, then at least the begin/finish progress events.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, InstallStreamEvent::Status(m) if m.contains("Resolving")))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, InstallStreamEvent::Progress(_)))
    );

    // The file landed as a content-addressed blob with a snapshot symlink + ref.
    let repo = root.join("models--org--Model");
    assert_eq!(std::fs::read(repo.join("blobs").join(&sha)).unwrap(), body);
    let snapshot = repo.join("snapshots/rev1/model.Q4_K_M.gguf");
    assert_eq!(std::fs::read(&snapshot).unwrap(), body);
    assert_eq!(
        std::fs::read_to_string(repo.join("refs/main")).unwrap(),
        "rev1"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn a_second_install_reuses_the_existing_blob() {
    let root = temp_root();
    let body = vec![1u8; 2048];
    let provider = provider(&root, &body);
    let plan = provider.plan("org/Model").await.expect("plan");
    drain(provider.install(plan)).await.expect("first install");

    // After the first install the plan reports nothing remaining.
    let replan = provider.plan("org/Model").await.expect("replan");
    assert_eq!(replan.remaining_bytes, Some(0));
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn search_delegates_to_the_hub() {
    let root = temp_root();
    let provider = provider(&root, b"x");
    let hits = provider.search("model", 10).await.expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reference, "org/Model");
    assert!(provider.supports_search());
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn an_invalid_reference_is_rejected() {
    let root = temp_root();
    let provider = provider(&root, b"x");
    match provider.plan("not a repo!!").await {
        Err(InstallError::ReferenceInvalid(_)) => {}
        other => panic!("expected reference-invalid, got {other:?}"),
    }
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn a_hostile_revision_is_rejected_and_nothing_is_written_outside_the_cache_root() {
    let root = temp_root();
    let body = vec![4u8; 256];
    let sha = sha_hex(&body);
    let siblings = format!(
        r#"{{"rfilename":"model.Q4_K_M.gguf","size":{size},"lfs":{{"size":{size},"oid":"{sha}"}}}}"#,
        size = body.len(),
    );
    let model_info =
        format!(r#"{{"id":"org/Model","sha":"../../evil","gated":false,"siblings":[{siblings}]}}"#);
    let search = r#"[{"id":"org/Model","downloads":5,"likes":1}]"#.to_owned();
    let transport: Arc<dyn InstallTransport> = Arc::new(HubMock {
        model_info,
        search,
        file_body: body.clone(),
    });
    let api = HFHubAPI::new(Arc::clone(&transport)).with_base_url("https://hf.test");
    let provider = HuggingFaceInstallProvider::new(api, transport, &root, "/home/me");

    let plan = provider.plan("org/Model").await.expect("plan");
    match drain(provider.install(plan)).await {
        Err(InstallError::ReferenceInvalid(_)) => {}
        other => panic!("expected reference-invalid, got {other:?}"),
    }

    // Nothing escaped the cache root: no sibling `evil` directory two levels up,
    // and the repo directory itself has no snapshot for the hostile revision.
    let outside = root
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("evil"));
    if let Some(outside) = outside {
        assert!(!outside.exists());
    }
    let repo = root.join("models--org--Model");
    assert!(!repo.join("snapshots").join("../../evil").exists());
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn a_gated_repo_without_a_token_requires_auth() {
    let root = temp_root();
    let body = vec![2u8; 512];
    let sha = sha_hex(&body);
    let siblings = format!(
        r#"{{"rfilename":"model.Q4_K_M.gguf","size":{size},"lfs":{{"size":{size},"oid":"{sha}"}}}}"#,
        size = body.len(),
    );
    // Gated + no token → requires_auth; gated + token → not.
    let no_token = provider_custom(&root, &body, true, &siblings, None);
    assert!(
        no_token
            .plan("org/Model")
            .await
            .expect("plan")
            .requires_auth
    );

    let root2 = temp_root();
    let with_token = provider_custom(&root2, &body, true, &siblings, Some("hf_secret"));
    assert!(
        !with_token
            .plan("org/Model")
            .await
            .expect("plan")
            .requires_auth
    );
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&root2).ok();
}

#[tokio::test]
async fn a_repo_with_no_downloadable_weights_is_rejected() {
    let root = temp_root();
    // Only a config file — no weights hedos knows how to fetch.
    let siblings = r#"{"rfilename":"config.json","size":10}"#;
    let provider = provider_custom(&root, b"{}", false, siblings, None);
    match provider.plan("org/Model").await {
        Err(InstallError::TransferFailed(message)) => assert!(message.contains("no model weights")),
        other => panic!("expected a no-weights transfer failure, got {other:?}"),
    }
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn availability_is_always_ready() {
    let root = temp_root();
    let provider = provider(&root, b"x");
    assert_eq!(
        provider.availability().await,
        kernel::install::InstallAvailability::Ready
    );
    std::fs::remove_dir_all(&root).ok();
}
