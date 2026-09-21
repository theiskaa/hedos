//! Tests for manifest runtimes as boot wires them: a `runtimes.d` manifest and
//! the shipped laya bundle both become adapters, neither bids until the settings
//! file holds an approval matching its files, an edit takes that approval back,
//! and a manifest that fails to load is reported rather than dropped.

mod support;

use kernel::records::{
    Capability, Modality, ModelRecord, ModelSource, ModelState, RuntimeId, SourceKind,
};
use kernel::registry::Registry;
use runtime::boot::{HedosDirs, build_kernel, runtimes_directory};
use runtime::facade::Kernel;
use runtime::settings::SettingsStore;
use support::TempDir;

const MANIFEST: &str = "id = \"judge\"\nmodalities = [\"text\"]\ncapabilities = [\"chat\"]\nexecution = \"stream\"\ndetect = { file = \"judge_config.json\" }\n[serve]\nentrypoint = \"main.py\"\n[env]\nlockfile = \"requirements.lock\"\n";

struct Fixture {
    root: TempDir,
    dirs: HedosDirs,
    settings: SettingsStore,
    model_id: String,
}

impl Fixture {
    /// A data dir whose registry holds one model nothing built in recognizes,
    /// and whose `runtimes.d` holds a manifest that detects it.
    fn new() -> Self {
        let root = TempDir::new();
        let dirs = HedosDirs {
            data: root.join("data"),
        };
        let runtime_dir = runtimes_directory(&dirs).join("judge");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        std::fs::write(runtime_dir.join("manifest.toml"), MANIFEST).unwrap();
        std::fs::write(runtime_dir.join("main.py"), "print('judge')\n").unwrap();
        std::fs::write(runtime_dir.join("requirements.lock"), "").unwrap();

        let model_dir = root.join("model");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("judge_config.json"), "{}").unwrap();
        let record = ModelRecord::new(
            "judge-model",
            Modality::unknown(),
            Vec::new(),
            ModelSource::new(SourceKind::folder(), &model_dir.to_string_lossy()),
        );
        let model_id = record.id.clone();
        let registry_dir = dirs.sub("registry");
        std::fs::create_dir_all(&registry_dir).unwrap();
        Registry::open(&registry_dir)
            .unwrap()
            .register(record)
            .unwrap();

        let settings = SettingsStore::new(root.join("hedos.toml"));
        Self {
            root,
            dirs,
            settings,
            model_id,
        }
    }

    /// Boot a kernel from the settings file as it stands and resolve the shelf.
    async fn booted(&self) -> (Kernel, ModelRecord) {
        let kernel = build_kernel(&self.dirs, &self.settings.load()).expect("build kernel");
        kernel.resolve().await.expect("resolve");
        let shelf = kernel.shelf().await;
        let record = shelf
            .iter()
            .find(|record| record.id == self.model_id)
            .expect("the model is on the shelf")
            .clone();
        (kernel, record)
    }

    fn entrypoint(&self) -> std::path::PathBuf {
        runtimes_directory(&self.dirs).join("judge").join("main.py")
    }
}

fn hash_of(kernel: &Kernel, id: &str) -> String {
    kernel
        .manifest_runtimes()
        .iter()
        .find(|manifest| manifest.id == id)
        .and_then(|manifest| manifest.content_hash.clone())
        .expect("the runtime loaded with a hash")
}

#[tokio::test]
async fn a_manifest_runtime_bids_only_after_a_real_approval_of_its_current_files() {
    let fixture = Fixture::new();

    let (kernel, parked) = fixture.booted().await;
    assert_eq!(parked.runtime.id, None, "unapproved, so nothing bids");
    assert_eq!(parked.state, ModelState::Unresolved);
    assert!(parked.capabilities.is_empty());

    fixture
        .settings
        .approve_runtime("judge", Some(&hash_of(&kernel, "judge")), false)
        .unwrap();
    let (_, served) = fixture.booted().await;
    assert_eq!(served.runtime.id, Some(RuntimeId::from("judge")));
    assert_eq!(served.state, ModelState::Ready);
    assert_eq!(served.modality, Modality::text());
    assert_eq!(served.capabilities, vec![Capability::chat()]);

    std::fs::write(fixture.entrypoint(), "print('something else')\n").unwrap();
    let (_, edited) = fixture.booted().await;
    assert_eq!(edited.runtime.id, None, "the approval was for other bytes");
    assert!(
        edited.capabilities.is_empty(),
        "and the capabilities the manifest lent go with it"
    );

    drop(fixture.root);
}

#[tokio::test]
async fn revoking_parks_the_model_again() {
    let fixture = Fixture::new();
    let (kernel, _) = fixture.booted().await;
    fixture
        .settings
        .approve_runtime("judge", Some(&hash_of(&kernel, "judge")), false)
        .unwrap();
    assert!(fixture.booted().await.1.runtime.id.is_some());

    fixture.settings.revoke_runtime("judge").unwrap();
    let (_, parked) = fixture.booted().await;
    assert_eq!(parked.runtime.id, None);
    assert_eq!(parked.modality, Modality::unknown());
}

#[tokio::test]
async fn deleting_an_approved_manifest_takes_back_what_it_lent() {
    let fixture = Fixture::new();
    let (kernel, _) = fixture.booted().await;
    fixture
        .settings
        .approve_runtime("judge", Some(&hash_of(&kernel, "judge")), false)
        .unwrap();
    assert!(fixture.booted().await.1.runtime.id.is_some());

    std::fs::remove_dir_all(runtimes_directory(&fixture.dirs).join("judge")).unwrap();
    let (_, orphaned) = fixture.booted().await;
    assert_eq!(orphaned.runtime.id, None);
    assert!(
        orphaned.capabilities.is_empty(),
        "no adapter is left to ask"
    );
}

#[tokio::test]
async fn a_manifest_cannot_declare_its_way_into_the_tools_capability() {
    let fixture = Fixture::new();
    let manifest = runtimes_directory(&fixture.dirs)
        .join("judge")
        .join("manifest.toml");
    std::fs::write(
        &manifest,
        MANIFEST.replace("[\"chat\"]", "[\"chat\", \"tools\"]"),
    )
    .unwrap();
    let (kernel, _) = fixture.booted().await;
    fixture
        .settings
        .approve_runtime("judge", Some(&hash_of(&kernel, "judge")), false)
        .unwrap();
    let (_, served) = fixture.booted().await;
    assert_eq!(served.capabilities, vec![Capability::chat()]);
}

#[tokio::test]
async fn the_shipped_judge_bundles_load_and_the_hand_driven_bundles_do_not() {
    let fixture = Fixture::new();
    let (kernel, _) = fixture.booted().await;
    let ids: Vec<&str> = kernel
        .manifest_runtimes()
        .iter()
        .map(|manifest| manifest.id.as_str())
        .collect();
    assert_eq!(ids, ["python:laya", "python:zerank", "judge"]);
    assert!(
        kernel.runtime_issues().is_empty(),
        "{:?}",
        kernel.runtime_issues()
    );
}

#[tokio::test]
async fn a_user_manifest_cannot_take_a_shipped_or_built_in_id() {
    let fixture = Fixture::new();
    for (folder, id) in [
        ("shadow-laya", "python:laya"),
        ("shadow-mlx", "python:mlx-lm"),
    ] {
        let dir = runtimes_directory(&fixture.dirs).join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.toml"),
            MANIFEST.replace("judge\"", &format!("{id}\"")),
        )
        .unwrap();
    }
    let (kernel, _) = fixture.booted().await;
    let issues = kernel.runtime_issues();
    assert_eq!(issues.len(), 2, "{issues:?}");
    assert!(issues.iter().all(|issue| issue.contains("is reserved")));
}

#[tokio::test]
async fn a_manifest_that_fails_to_load_is_reported_by_a_scan() {
    let fixture = Fixture::new();
    std::fs::write(
        runtimes_directory(&fixture.dirs).join("broken.toml"),
        "id = \"broken\"\n",
    )
    .unwrap();
    let (kernel, _) = fixture.booted().await;
    let summary = kernel.discover(Vec::new()).await.expect("discover");
    assert!(
        summary
            .issues
            .iter()
            .any(|issue| issue.starts_with("runtimes.d/broken.toml")),
        "{:?}",
        summary.issues
    );
}

#[tokio::test]
async fn a_cross_encoder_the_shelf_once_called_an_embedder_is_served_by_zerank_once_approved() {
    let fixture = Fixture::new();
    let model_dir = fixture.root.join("zerank");
    std::fs::create_dir_all(&model_dir).unwrap();
    for (file, contents) in [
        ("config.json", r#"{"architectures":["Qwen3ForCausalLM"]}"#),
        ("model.safetensors", "weights"),
        ("tokenizer.json", "{}"),
        (
            "config_sentence_transformers.json",
            r#"{"model_type":"CrossEncoder"}"#,
        ),
        (
            "chat_template.jinja",
            "{%- set zerank_query = messages | selectattr(\"role\", \"eq\", \"query\") -%}",
        ),
    ] {
        std::fs::write(model_dir.join(file), contents).unwrap();
    }
    // What a shelf written before cross-encoders were told apart holds for it.
    let record = ModelRecord::new(
        "zerank-2-reranker",
        Modality::embedding(),
        vec![Capability::embed()],
        ModelSource::new(SourceKind::folder(), &model_dir.to_string_lossy()),
    );
    let zerank_id = record.id.clone();
    Registry::open(&fixture.dirs.sub("registry"))
        .unwrap()
        .register(record)
        .unwrap();
    let zerank = |shelf: &[ModelRecord]| {
        shelf
            .iter()
            .find(|record| record.id == zerank_id)
            .expect("zerank is on the shelf")
            .clone()
    };

    let (kernel, _) = fixture.booted().await;
    let unapproved = zerank(&kernel.shelf().await);
    assert_ne!(
        unapproved.runtime.id,
        Some(RuntimeId::from("python:embeddings"))
    );
    assert_eq!(unapproved.state, ModelState::Unresolved);

    fixture
        .settings
        .approve_runtime(
            "python:zerank",
            Some(&hash_of(&kernel, "python:zerank")),
            false,
        )
        .unwrap();
    let (kernel, _) = fixture.booted().await;
    let served = zerank(&kernel.shelf().await);
    assert_eq!(served.runtime.id, Some(RuntimeId::from("python:zerank")));
    assert_eq!(served.modality, Modality::text());
    assert_eq!(
        served.capabilities,
        vec![Capability::chat(), Capability::judge()]
    );
}

/// A zerank-shaped snapshot, registered the way a shelf written before
/// cross-encoders were told apart holds it: an embedder, on the embeddings
/// runtime. Returns the record's id.
fn stale_zerank(fixture: &Fixture) -> String {
    let model_dir = fixture.root.join("zerank");
    std::fs::create_dir_all(&model_dir).unwrap();
    for (file, contents) in [
        ("config.json", r#"{"architectures":["Qwen3ForCausalLM"]}"#),
        ("model.safetensors", "weights"),
        ("tokenizer.json", "{}"),
        (
            "config_sentence_transformers.json",
            r#"{"model_type":"CrossEncoder"}"#,
        ),
        (
            "chat_template.jinja",
            "{%- set zerank_query = messages | selectattr(\"role\", \"eq\", \"query\") -%}",
        ),
    ] {
        std::fs::write(model_dir.join(file), contents).unwrap();
    }
    let mut record = ModelRecord::new(
        "zerank-2-reranker",
        Modality::embedding(),
        vec![Capability::embed()],
        ModelSource::new(SourceKind::folder(), &model_dir.to_string_lossy()),
    );
    record.runtime.id = Some(RuntimeId::from("python:embeddings"));
    record.state = ModelState::Ready;
    let id = record.id.clone();
    Registry::open(&fixture.dirs.sub("registry"))
        .unwrap()
        .register(record)
        .unwrap();
    id
}

fn on_shelf(shelf: &[ModelRecord], id: &str) -> ModelRecord {
    shelf
        .iter()
        .find(|record| record.id == id)
        .expect("the model is on the shelf")
        .clone()
}

#[tokio::test]
async fn a_stale_embedder_row_is_taken_back_on_first_sight_of_the_shelf_without_a_scan() {
    let fixture = Fixture::new();
    let id = stale_zerank(&fixture);

    // No resolve and no discover: what `hedos serve` does before its first request.
    let kernel = build_kernel(&fixture.dirs, &fixture.settings.load()).expect("build kernel");
    let unapproved = on_shelf(&kernel.shelf().await, &id);
    assert_eq!(unapproved.modality, Modality::text());
    assert!(unapproved.capabilities.is_empty());
    assert_eq!(unapproved.runtime.id, None);
    assert_eq!(unapproved.state, ModelState::Unresolved);

    fixture
        .settings
        .approve_runtime(
            "python:zerank",
            Some(&hash_of(&kernel, "python:zerank")),
            false,
        )
        .unwrap();
    let stale_again = stale_zerank(&fixture);
    assert_eq!(stale_again, id);
    let kernel = build_kernel(&fixture.dirs, &fixture.settings.load()).expect("build kernel");
    let served = on_shelf(&kernel.shelf().await, &id);
    assert_eq!(served.runtime.id, Some(RuntimeId::from("python:zerank")));
    assert_eq!(
        served.capabilities,
        vec![Capability::chat(), Capability::judge()]
    );
}
