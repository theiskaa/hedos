//! Tests for the Hugging Face hub client, driven by a mock transport.

use std::sync::{Arc, Mutex};

use kernel::install::InstallError;
use runtime::install::transport::TransportFuture;
use runtime::install::{HFHubAPI, InstallRequest, InstallResponse, InstallTransport};

/// A transport that records the request and returns a canned response.
struct MockTransport {
    status: u16,
    body: &'static str,
    last: Arc<Mutex<Option<InstallRequest>>>,
}

impl MockTransport {
    fn new(status: u16, body: &'static str) -> (Arc<Self>, Arc<Mutex<Option<InstallRequest>>>) {
        let last = Arc::new(Mutex::new(None));
        (
            Arc::new(Self {
                status,
                body,
                last: Arc::clone(&last),
            }),
            last,
        )
    }
}

impl InstallTransport for MockTransport {
    fn fetch(&self, request: InstallRequest) -> TransportFuture {
        *self.last.lock().unwrap() = Some(request);
        let response = InstallResponse {
            status: self.status,
            body: self.body.as_bytes().to_vec(),
        };
        Box::pin(async move { Ok(response) })
    }
}

fn hub(transport: Arc<MockTransport>) -> HFHubAPI {
    HFHubAPI::new(transport).with_base_url("https://hf.test")
}

#[tokio::test]
async fn search_parses_hits_and_the_query() {
    let body = r#"[
        {"id":"meta-llama/Llama-3","downloads":1000,"likes":50,"lastModified":"2024-01-15T10:30:00.000Z"},
        {"id":"bare-model","downloads":null,"likes":null,"lastModified":null}
    ]"#;
    let (transport, last) = MockTransport::new(200, body);
    let hits = hub(transport).search("llama", 20).await.expect("search");

    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].reference, "meta-llama/Llama-3");
    assert_eq!(hits[0].name, "Llama-3"); // last path segment
    assert_eq!(hits[0].downloads, Some(1000));
    assert_eq!(hits[0].likes, Some(50));
    assert_eq!(hits[0].updated_at, Some(1_705_314_600_000));
    // A bare id keeps itself as the name.
    assert_eq!(hits[1].name, "bare-model");
    assert_eq!(hits[1].downloads, None);
    assert_eq!(hits[1].updated_at, None);

    // The request carried the search/limit/sort query params.
    let url = last.lock().unwrap().clone().unwrap().url;
    assert!(url.contains("/api/models"));
    assert!(url.contains("search=llama"));
    assert!(url.contains("limit=20"));
    assert!(url.contains("sort=downloads"));
}

#[tokio::test]
async fn a_token_becomes_a_bearer_header() {
    let (transport, last) = MockTransport::new(200, "[]");
    let _ = hub(transport)
        .with_token(Some("hf_secret".to_owned()))
        .search("q", 5)
        .await
        .expect("search");
    let request = last.lock().unwrap().clone().unwrap();
    assert!(
        request
            .headers
            .iter()
            .any(|(name, value)| name == "authorization" && value == "Bearer hf_secret")
    );
}

#[tokio::test]
async fn model_info_parses_metadata_and_reduces_sibling_sizes() {
    let body = r#"{
        "id":"org/Model",
        "sha":"abc123",
        "gated":"auto",
        "siblings":[
            {"rfilename":"model.safetensors","size":500},
            {"rfilename":"weights.bin","lfs":{"size":900}},
            {"rfilename":"config.json"}
        ]
    }"#;
    let (transport, last) = MockTransport::new(200, body);
    let info = hub(transport).model_info("org/model").await.expect("info");

    assert_eq!(info.repo, "org/Model"); // canonical id (has a slash) wins
    assert_eq!(info.sha, Some("abc123".to_owned()));
    assert!(info.gated); // "auto" != "false" → gated
    assert_eq!(info.siblings.len(), 3);
    // `size` is used directly...
    assert_eq!(info.siblings[0].bytes, Some(500));
    // ...else the LFS pointer's size (Swift `size ?? lfs.size`)...
    assert_eq!(info.siblings[1].bytes, Some(900));
    // ...else None.
    assert_eq!(info.siblings[2].bytes, None);

    // The request asked for blobs.
    let url = last.lock().unwrap().clone().unwrap().url;
    assert!(url.contains("/api/models/org/model"));
    assert!(url.contains("blobs=true"));
}

#[tokio::test]
async fn gated_false_and_bool_forms_are_handled() {
    let (transport, _) = MockTransport::new(200, r#"{"gated":false,"siblings":[]}"#);
    assert!(!hub(transport).model_info("org/m").await.unwrap().gated);

    let (transport, _) = MockTransport::new(200, r#"{"gated":true,"siblings":[]}"#);
    assert!(hub(transport).model_info("org/m").await.unwrap().gated);

    let (transport, _) = MockTransport::new(200, r#"{"gated":"false","siblings":[]}"#);
    assert!(!hub(transport).model_info("org/m").await.unwrap().gated);
}

#[tokio::test]
async fn model_info_maps_http_errors() {
    let (transport, _) = MockTransport::new(404, "not found");
    assert!(matches!(
        hub(transport).model_info("org/missing").await,
        Err(InstallError::ReferenceNotFound(_))
    ));

    let (transport, _) = MockTransport::new(401, "no");
    assert!(matches!(
        hub(transport).model_info("org/gated").await,
        Err(InstallError::AuthRequired(_))
    ));

    let (transport, _) = MockTransport::new(500, "boom");
    assert!(matches!(
        hub(transport).model_info("org/m").await,
        Err(InstallError::TransferFailed(_))
    ));
}

#[tokio::test]
async fn search_surfaces_a_non_200_as_a_transfer_failure() {
    let (transport, _) = MockTransport::new(503, "down");
    assert!(matches!(
        hub(transport).search("q", 5).await,
        Err(InstallError::TransferFailed(_))
    ));
}

#[test]
fn resolve_url_builds_the_download_path() {
    let (transport, _) = MockTransport::new(200, "");
    let url = hub(transport).resolve_url("org/model", "main", "sub/dir/model.safetensors");
    assert_eq!(
        url,
        "https://hf.test/org/model/resolve/main/sub/dir/model.safetensors"
    );
}

#[test]
fn resolve_url_percent_encodes_special_characters() {
    let (transport, _) = MockTransport::new(200, "");
    let url = hub(transport).resolve_url("org/model", "main", "my file.bin");
    // The space is encoded, not left literal.
    assert!(url.ends_with("/resolve/main/my%20file.bin"), "{url}");
}

#[tokio::test]
async fn an_lfs_oid_becomes_the_sibling_sha256() {
    let body = r#"{"siblings":[
        {"rfilename":"model.safetensors","lfs":{"size":900,"oid":"deadbeef"}},
        {"rfilename":"config.json","size":10}
    ]}"#;
    let (transport, _) = MockTransport::new(200, body);
    let info = hub(transport).model_info("org/m").await.unwrap();
    // The LFS file carries its content hash; the plain file has none.
    assert_eq!(info.siblings[0].sha256, Some("deadbeef".to_owned()));
    assert_eq!(info.siblings[0].bytes, Some(900));
    assert_eq!(info.siblings[1].sha256, None);
}

#[tokio::test]
async fn a_size_wins_over_an_lfs_size() {
    let body = r#"{"siblings":[{"rfilename":"w.bin","size":100,"lfs":{"size":999}}]}"#;
    let (transport, _) = MockTransport::new(200, body);
    let info = hub(transport).model_info("org/m").await.unwrap();
    assert_eq!(info.siblings[0].bytes, Some(100));
}

#[tokio::test]
async fn a_null_or_slashless_id_falls_back_to_the_requested_repo() {
    // No `id` field → the passed repo is used.
    let (transport, _) = MockTransport::new(200, r#"{"siblings":[]}"#);
    assert_eq!(
        hub(transport).model_info("org/m").await.unwrap().repo,
        "org/m"
    );
    // A slashless `id` → also the passed repo.
    let (transport, _) = MockTransport::new(200, r#"{"id":"m","siblings":[]}"#);
    assert_eq!(
        hub(transport).model_info("org/m").await.unwrap().repo,
        "org/m"
    );
}

#[tokio::test]
async fn an_absent_or_numeric_gated_field_is_not_gated() {
    // Absent → false.
    let (transport, _) = MockTransport::new(200, r#"{"siblings":[]}"#);
    assert!(!hub(transport).model_info("org/m").await.unwrap().gated);
    // An unexpected numeric shape → false, not a decode error.
    let (transport, _) = MockTransport::new(200, r#"{"gated":0,"siblings":[]}"#);
    assert!(!hub(transport).model_info("org/m").await.unwrap().gated);
}

#[tokio::test]
async fn malformed_json_is_a_transfer_failure_not_a_panic() {
    let (transport, _) = MockTransport::new(200, "{ not json");
    assert!(matches!(
        hub(transport).model_info("org/m").await,
        Err(InstallError::TransferFailed(_))
    ));
}

#[tokio::test]
async fn a_forbidden_status_is_auth_required() {
    let (transport, _) = MockTransport::new(403, "no");
    assert!(matches!(
        hub(transport).model_info("org/gated").await,
        Err(InstallError::AuthRequired(_))
    ));
}

#[tokio::test]
async fn a_trailing_slash_id_names_the_last_real_segment() {
    let (transport, _) = MockTransport::new(200, r#"[{"id":"org/"}]"#);
    let hits = hub(transport).search("q", 5).await.unwrap();
    assert_eq!(hits[0].name, "org");
}
