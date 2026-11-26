//! Tests for the `Kernel` facade's discover→resolve→explain surface: a scanner's
//! findings are reconciled into the registry and resolved to runtimes end-to-end.

mod support;

use kernel::Registry;
use kernel::artifacts::ArtifactStore;
use kernel::discovery::{DiscoveredModel, ScanResult, StoreScanner};
use kernel::jobs::JobHistoryStore;
use kernel::records::{
    Capability, Modality, ModelRecord, ModelSource, ModelState, Resolution, RuntimeId, SourceKind,
};
use runtime::adapters::OllamaAdapter;
use runtime::boot::{HedosDirs, build_kernel};
use runtime::facade::{Kernel, RegisteredAdapter};
use runtime::governor::{GovernorConfig, MemoryGovernor};
use runtime::settings::Settings;
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

    // Explain did not resolve the record: it is still unresolved in the store.
    // Read the store itself, since a shelf read resolves Ollama's models.
    let stored = Registry::open(dir.path()).expect("registry");
    assert_eq!(stored.list()[0].runtime.id, None);
}

#[tokio::test]
async fn explain_on_an_empty_registry_is_empty() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, None);
    assert!(kernel.explain().await.is_empty());
}

/// An adapter that bids on what identification reads as an MLX weight
/// directory, and on nothing else.
struct MlxOnly {
    id: RuntimeId,
}

impl runtime::adapters::RuntimeAdapter for MlxOnly {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn can_serve(&self, _record: &ModelRecord, _capability: &kernel::records::Capability) -> bool {
        true
    }

    fn bid(
        &self,
        _record: &ModelRecord,
        identified: &kernel::resolution::IdentifiedModel,
    ) -> Option<kernel::resolution::RuntimeBid> {
        (identified.format == kernel::resolution::ModelFormat::MlxSafetensors)
            .then(|| kernel::resolution::RuntimeBid::new(kernel::records::RunTier::Native, 15))
    }

    fn invoke(
        &self,
        _record: &ModelRecord,
        _capability: kernel::records::Capability,
        _payload: kernel::records::JsonValue,
    ) -> runtime::adapters::ChunkStream {
        runtime::adapters::ChunkStream::failed(runtime::adapters::RuntimeError::Failed(
            "unused".into(),
        ))
    }
}

#[tokio::test]
async fn an_index_of_a_variant_nobody_fetched_leaves_an_mlx_repo_resolved() {
    let dir = TempDir::new();
    let hub = dir.path().join("hub");
    let repo = hub.join("models--mlx-community--tiny");
    let snapshot = repo.join("snapshots").join("r1");
    std::fs::create_dir_all(&snapshot).unwrap();
    std::fs::create_dir_all(repo.join("refs")).unwrap();
    std::fs::write(repo.join("refs").join("main"), "r1").unwrap();
    std::fs::write(
        snapshot.join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"],"model_type":"llama","quantization":{"group_size":64,"bits":4}}"#,
    )
    .unwrap();
    std::fs::write(snapshot.join("tokenizer_config.json"), b"{}").unwrap();
    std::fs::write(snapshot.join("model.safetensors"), vec![0u8; 64]).unwrap();
    std::fs::write(
        snapshot.join("model.safetensors.index.fp16.json"),
        br#"{"weight_map":{"a":"model.fp16-00001-of-00002.safetensors","b":"model.fp16-00002-of-00002.safetensors"}}"#,
    )
    .unwrap();

    let mlx = RuntimeId::mlx_lm();
    let kernel = Kernel::new(
        Registry::open(&dir.path().join("shelf")).expect("registry"),
        ArtifactStore::new(dir.path()),
        Arc::new(MemoryGovernor::new(GovernorConfig::with_total_mb(262_144))),
        JobHistoryStore::with_default_limit(dir.path()),
        vec![RegisteredAdapter::streaming(Arc::new(MlxOnly {
            id: mlx.clone(),
        }))],
    );

    kernel
        .discover(vec![Box::new(kernel::discovery::HFCacheScanner::single(
            &hub,
        ))])
        .await
        .expect("discover");

    let shelf = kernel.shelf().await;
    let tiny = shelf.iter().find(|m| m.name == "tiny").expect("tiny");
    assert!(!tiny.downloading);
    assert_eq!(tiny.runtime.id, Some(mlx));
    assert_eq!(tiny.state, ModelState::Ready);
}

/// A cached Hugging Face repo `name` under `hub` at revision `r1`, holding
/// `config` beside a plain (or, with `mlx`, an MLX) safetensors weight and the
/// ModelOpt quantization sidecar.
fn cached_checkpoint(hub: &std::path::Path, name: &str, config: serde_json::Value, mlx: bool) {
    let repo = hub.join(format!("models--fixture--{name}"));
    let snapshot = repo.join("snapshots/r1");
    std::fs::create_dir_all(&snapshot).unwrap();
    std::fs::create_dir_all(repo.join("refs")).unwrap();
    std::fs::write(repo.join("refs/main"), "r1").unwrap();
    std::fs::write(snapshot.join("config.json"), config.to_string()).unwrap();
    std::fs::write(snapshot.join("hf_quant_config.json"), b"{}").unwrap();
    let header = if mlx {
        &br#"{"__metadata__":{"format":"mlx"}}"#[..]
    } else {
        &br#"{"__metadata__":{"format":"pt"}}"#[..]
    };
    let mut weight = (header.len() as u64).to_le_bytes().to_vec();
    weight.extend(header);
    std::fs::write(snapshot.join("model.safetensors"), weight).unwrap();
}

/// The unresolved record an older shelf kept for the cached repo `name`.
fn stranded_record(hub: &std::path::Path, name: &str) -> ModelRecord {
    let repo = hub.join(format!("models--fixture--{name}"));
    let mut source = ModelSource::new(SourceKind::huggingface_cache(), &repo.to_string_lossy());
    source.reference = Some("r1".to_owned());
    let mut record = ModelRecord::new(
        name,
        Modality::text(),
        vec![
            Capability::chat(),
            Capability::complete(),
            Capability::see(),
        ],
        source,
    );
    record.state = ModelState::Unresolved;
    record.primary_weight_path = Some(
        repo.join("snapshots/r1/model.safetensors")
            .to_string_lossy()
            .into_owned(),
    );
    record
}

fn inkling_mini(is_varloop: bool) -> serde_json::Value {
    serde_json::json!({
        "architectures": ["Inkling2LoopForConditionalGeneration"],
        "model_type": "inkling2_loop",
        "vision_config": {},
        "quantization_config": {"quant_method": "modelopt"},
        "text_config": {"loop_config": {"is_varloop": is_varloop}},
    })
}

#[tokio::test]
async fn original_inkling_2_checkpoints_resolve_on_first_shelf_read_and_on_discovery() {
    let apple_silicon = cfg!(all(target_os = "macos", target_arch = "aarch64"));
    let dir = TempDir::new();
    let hub = dir.join("hub");
    let small = serde_json::json!({
        "architectures": ["Inkling2ForConditionalGeneration"],
        "model_type": "inkling2",
        "vision_config": {},
    });
    let other = serde_json::json!({
        "architectures": ["Qwen2VLForConditionalGeneration"],
        "model_type": "qwen2_vl",
        "vision_config": {},
    });
    for (name, config, mlx) in [
        ("Mini", inkling_mini(false), false),
        ("MiniPinned", inkling_mini(false), false),
        ("MiniPulling", inkling_mini(false), false),
        ("Small", small, false),
        ("Variable", inkling_mini(true), false),
        ("OtherVision", other, false),
        ("Mini4bit", inkling_mini(false), true),
        ("VariableMlx", inkling_mini(true), true),
    ] {
        cached_checkpoint(&hub, name, config, mlx);
    }

    let dirs = HedosDirs {
        data: dir.join("data"),
    };
    let mut registry = Registry::open(&dirs.sub("registry")).unwrap();
    let mini = stranded_record(&hub, "Mini");
    let other = stranded_record(&hub, "OtherVision");
    let mut pinned = stranded_record(&hub, "MiniPinned");
    pinned.runtime.resolved = Resolution::User;
    let mut pulling = stranded_record(&hub, "MiniPulling");
    pulling.downloading = true;
    for record in [&mini, &other, &pinned, &pulling] {
        registry.register(record.clone()).unwrap();
    }
    drop(registry);

    let ready_on_mlx_vlm = |record: &ModelRecord| {
        if apple_silicon {
            assert_eq!(record.state, ModelState::Ready, "{}", record.name);
            assert_eq!(
                record.runtime.id,
                Some(RuntimeId::mlx_vlm()),
                "{}",
                record.name
            );
            assert!(record.capabilities.contains(&Capability::chat()));
            assert!(record.capabilities.contains(&Capability::see()));
        } else {
            assert_eq!(record.state, ModelState::Unresolved, "{}", record.name);
        }
    };

    let kernel = build_kernel(&dirs, &Settings::default()).unwrap();
    let shelf = kernel.shelf().await;
    let find = |id: &str| shelf.iter().find(|model| model.id == id).unwrap().clone();
    let migrated = find(&mini.id);
    ready_on_mlx_vlm(&migrated);
    assert_eq!(migrated.source, mini.source);
    assert_eq!(migrated.primary_weight_path, mini.primary_weight_path);
    assert_eq!(find(&other.id).state, ModelState::Unresolved);
    assert_eq!(find(&pinned.id), pinned);
    assert_eq!(find(&pulling.id), pulling);
    assert_eq!(
        Registry::open(&dirs.sub("registry"))
            .unwrap()
            .migration_level(),
        4
    );
    assert!(Arc::ptr_eq(&shelf, &kernel.shelf().await));

    let fresh = build_kernel(
        &HedosDirs {
            data: dir.join("fresh"),
        },
        &Settings::default(),
    )
    .unwrap();
    fresh
        .discover(vec![Box::new(kernel::discovery::HFCacheScanner::single(
            &hub,
        ))])
        .await
        .unwrap();
    let shelf = fresh.shelf().await;
    let named = |name: &str| shelf.iter().find(|model| model.name == name).unwrap();
    ready_on_mlx_vlm(named("Mini"));
    ready_on_mlx_vlm(named("Small"));
    assert_eq!(named("Mini4bit").runtime.id, Some(RuntimeId::mlx_vlm()));
    for name in ["Variable", "VariableMlx", "OtherVision"] {
        assert_eq!(named(name).state, ModelState::Unresolved, "{name}");
    }
}
