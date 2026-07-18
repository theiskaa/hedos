//! Tests for the `Kernel` facade's discover→resolve→explain surface: a scanner's
//! findings are reconciled into the registry and resolved to runtimes end-to-end.

mod support;

use kernel::Registry;
use kernel::artifacts::ArtifactStore;
use kernel::discovery::{DiscoveredModel, ScanResult, StoreScanner};
use kernel::jobs::JobHistoryStore;
use kernel::records::{
    Modality, ModelRecord, ModelSource, ModelState, Resolution, RuntimeId, SourceKind,
};
use runtime::adapters::OllamaAdapter;
use runtime::facade::{Kernel, RegisteredAdapter};
use runtime::governor::{GovernorConfig, MemoryGovernor};
use std::sync::Arc;
use support::TempDir;

/// A scanner that returns a fixed [`ScanResult`].
struct FakeScanner {
    result: ScanResult,
}

impl FakeScanner {
    fn with_models(models: Vec<DiscoveredModel>) -> Box<Self> {
        Box::new(Self {
            result: ScanResult {
                discovered: models,
                ..Default::default()
            },
        })
    }
}

impl StoreScanner for FakeScanner {
    fn kinds(&self) -> Vec<SourceKind> {
        vec![SourceKind::ollama()]
    }

    fn scan(&self) -> ScanResult {
        self.result.clone()
    }
}

fn discovered(name: &str, kind: SourceKind, path: &str) -> DiscoveredModel {
    DiscoveredModel::new(name, ModelSource::new(kind, path))
}

/// A kernel wired with the real Ollama adapter (which bids on ollama-store models)
/// over an empty registry, optionally pre-seeded with `record`.
fn kernel(dir: &TempDir, record: Option<ModelRecord>) -> Kernel {
    let mut registry = Registry::open(dir.path()).expect("registry");
    if let Some(record) = record {
        registry.register(record).expect("register");
    }
    let artifacts = ArtifactStore::new(dir.path());
    let governor = Arc::new(MemoryGovernor::new(GovernorConfig::with_total_mb(262_144)));
    let history = JobHistoryStore::with_default_limit(dir.path());
    let adapters = vec![RegisteredAdapter::streaming(Arc::new(OllamaAdapter::new()))];
    Kernel::new(registry, artifacts, governor, history, adapters)
}

fn ollama_record(name: &str) -> ModelRecord {
    ModelRecord::new(
        name,
        Modality::text(),
        Vec::new(),
        ModelSource::new(SourceKind::ollama(), "/store/manifest"),
    )
}

#[tokio::test]
async fn discover_reconciles_findings_and_resolves_them_to_a_runtime() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, None);
    let scanner = FakeScanner::with_models(vec![discovered(
        "llama3.2:latest",
        SourceKind::ollama(),
        "/store/manifest",
    )]);

    let summary = kernel.discover(vec![scanner]).await.expect("discover");
    assert_eq!(summary.total_count, 1);

    // The discovered model landed on the shelf, resolved to the Ollama runtime.
    let shelf = kernel.shelf().await;
    assert_eq!(shelf.len(), 1);
    assert_eq!(shelf[0].name, "llama3.2:latest");
    assert_eq!(shelf[0].runtime.id, Some(RuntimeId::ollama()));
    assert_eq!(shelf[0].state, ModelState::Ready);
}

#[tokio::test]
async fn discover_leaves_a_model_no_adapter_bids_on_unresolved() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, None);
    // A biddable ollama model and a loose file the Ollama adapter won't bid on.
    let scanner = FakeScanner::with_models(vec![
        discovered("qwen", SourceKind::ollama(), "/store/manifest"),
        discovered("mystery", SourceKind::file(), "/loose/model.bin"),
    ]);

    let summary = kernel.discover(vec![scanner]).await.expect("discover");
    assert_eq!(summary.total_count, 2);

    let shelf = kernel.shelf().await;
    let ollama = shelf.iter().find(|m| m.name == "qwen").expect("qwen");
    assert_eq!(ollama.runtime.id, Some(RuntimeId::ollama()));
    assert_eq!(ollama.state, ModelState::Ready);
    let file = shelf.iter().find(|m| m.name == "mystery").expect("mystery");
    assert_eq!(file.runtime.id, None);
    assert_eq!(file.state, ModelState::Unresolved);
}

#[tokio::test]
async fn discover_surfaces_scanner_issues_and_failed_kinds_in_the_summary() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, None);
    let scanner = Box::new(FakeScanner {
        result: ScanResult {
            discovered: Vec::new(),
            issues: vec!["a blob was unreadable".to_owned()],
            failed_kinds: vec![SourceKind::ollama()],
        },
    });

    let summary = kernel.discover(vec![scanner]).await.expect("discover");
    assert!(summary.issues.iter().any(|i| i.contains("unreadable")));
    // The service also synthesizes an issue for the wholesale-failed kind.
    assert!(summary.failed_kinds.contains(&SourceKind::ollama()));
}

#[tokio::test]
async fn resolve_leaves_a_user_pinned_record_untouched() {
    let dir = TempDir::new();
    let mut pinned = ollama_record("pinned");
    // The user pinned this to the (live) Ollama runtime — resolution must not
    // re-decide it.
    pinned.runtime.resolved = Resolution::User;
    pinned.runtime.id = Some(RuntimeId::ollama());
    let kernel = kernel(&dir, Some(pinned));

    assert!(kernel.resolve().await.expect("resolve").is_empty());
    let shelf = kernel.shelf().await;
    assert_eq!(shelf[0].runtime.resolved, Resolution::User);
}

#[tokio::test]
async fn resolve_writes_the_runtime_onto_an_already_registered_record() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, Some(ollama_record("qwen")));

    let changed = kernel.resolve().await.expect("resolve");
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].runtime.id, Some(RuntimeId::ollama()));

    // A second resolve is a no-op — the record is already resolved.
    assert!(kernel.resolve().await.expect("resolve").is_empty());
}

#[tokio::test]
async fn explain_reports_the_bids_without_mutating() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, Some(ollama_record("qwen")));

    let explanations = kernel.explain().await;
    assert_eq!(explanations.len(), 1);
    assert_eq!(explanations[0].winner(), Some(&RuntimeId::ollama()));

    // Explain did not resolve the record — it is still unresolved on the shelf.
    let shelf = kernel.shelf().await;
    assert_eq!(shelf[0].runtime.id, None);
}

#[tokio::test]
async fn explain_on_an_empty_registry_is_empty() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, None);
    assert!(kernel.explain().await.is_empty());
}
