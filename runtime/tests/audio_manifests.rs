//! Manifest runtimes for models that ship their own code: a YuE2-style
//! manifest detects its model by `model_type`, lends it the `audio` capability,
//! the job execution mode and its parameter schema once approved, and takes
//! them back when revoked. The model folders hold the real `config.json` and
//! card captured under `kernel/tests/fixtures/audio`.

mod support;

use std::path::{Path, PathBuf};

use kernel::records::{
    Capability, ExecutionMode, Modality, ModelRecord, ModelSource, ModelState, RuntimeId,
    SourceKind,
};
use kernel::registry::Registry;
use runtime::boot::{HedosDirs, build_kernel, runtimes_directory};
use runtime::facade::Kernel;
use runtime::settings::SettingsStore;
use support::TempDir;

const YUE2_MANIFEST: &str = r#"id = "python:song"
modalities = ["audio"]
capabilities = ["audio"]
execution = "job"
detect = { model_type = "yue2" }

[[params]]
key = "lyrics"
type = "string"

[[params]]
key = "cot"
type = "enum"
values = ["full", "melody", "off"]
default = "full"

[[params]]
key = "seed"
type = "int"

[companions]
vae = { repo = "m-a-p/YuE2-Vae" }

[serve]
entrypoint = "main.py"

[env]
lockfile = "requirements.lock"
"#;

const VOICE_MANIFEST: &str = r#"id = "python:voice"
modalities = ["speech"]
capabilities = ["speak"]
execution = "stream"
detect = { model_type = "llama" }

[[params]]
key = "emotion"
type = "string"

[serve]
entrypoint = "main.py"

[env]
lockfile = "requirements.lock"
"#;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../kernel/tests/fixtures/audio")
        .join(name)
}

struct Shelf {
    root: TempDir,
    dirs: HedosDirs,
    settings: SettingsStore,
    model_id: String,
}

impl Shelf {
    /// A data dir with `manifest` in `runtimes.d` under `runtime`, and a
    /// registry holding one folder model made of `fixture_name`'s files.
    fn new(runtime: &str, manifest: &str, fixture_name: &str) -> Self {
        let root = TempDir::new();
        let dirs = HedosDirs {
            data: root.join("data"),
        };
        // An empty manifest leaves `runtimes.d` empty: only the shipped
        // runtimes load.
        if !manifest.is_empty() {
            let runtime_dir = runtimes_directory(&dirs).join(runtime);
            std::fs::create_dir_all(&runtime_dir).unwrap();
            std::fs::write(runtime_dir.join("manifest.toml"), manifest).unwrap();
            std::fs::write(runtime_dir.join("main.py"), "print('stand-in')\n").unwrap();
            std::fs::write(runtime_dir.join("requirements.lock"), "").unwrap();
        }

        let model_dir = root.join("model");
        std::fs::create_dir_all(&model_dir).unwrap();
        for entry in std::fs::read_dir(fixture(fixture_name)).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), model_dir.join(entry.file_name())).unwrap();
        }
        std::fs::write(model_dir.join("model.safetensors"), b"weights").unwrap();
        let mut record = ModelRecord::new(
            fixture_name,
            Modality::unknown(),
            Vec::new(),
            ModelSource::new(SourceKind::folder(), &model_dir.to_string_lossy()),
        );
        record.serving_bytes = Some(7_261_441_640);
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

    /// Put a pulled hub repo `repo` on the shelf, serving `bytes`, with an
    /// empty snapshot directory as a stand-in for its files.
    fn shelve_hub_repo(&self, repo: &str, bytes: i64) -> PathBuf {
        let hub = self
            .root
            .join("hub")
            .join(format!("models--{}", repo.replace('/', "--")));
        let snapshot = hub.join("snapshots").join("abc");
        std::fs::create_dir_all(&snapshot).unwrap();
        let mut source = ModelSource::new(SourceKind::huggingface_cache(), &hub.to_string_lossy());
        source.repo = Some(repo.to_owned());
        source.reference = Some("abc".to_owned());
        let mut record = ModelRecord::new(repo, Modality::audio(), Vec::new(), source);
        record.serving_bytes = Some(bytes);
        Registry::open(&self.dirs.sub("registry"))
            .unwrap()
            .register(record)
            .unwrap();
        std::fs::canonicalize(snapshot).unwrap()
    }

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

    async fn approve(&self, id: &str) {
        let (kernel, _) = self.booted().await;
        let hash = kernel
            .manifest_runtimes()
            .iter()
            .find(|manifest| manifest.id == id)
            .and_then(|manifest| manifest.content_hash.clone())
            .expect("the runtime loaded with a hash");
        self.settings
            .approve_runtime(id, Some(&hash), false)
            .unwrap();
    }
}

fn keys(record: &ModelRecord) -> Vec<&str> {
    record.params.iter().map(|spec| spec.key.as_str()).collect()
}

#[tokio::test]
async fn an_approved_audio_manifest_serves_yue2_as_a_job_with_its_params() {
    let shelf = Shelf::new("song", YUE2_MANIFEST, "yue2-3b");

    let (kernel, parked) = shelf.booted().await;
    assert_eq!(parked.runtime.id, None);
    assert_eq!(parked.modality, Modality::audio());
    assert!(parked.capabilities.is_empty(), "never chat, even unserved");
    let manifest = kernel
        .manifest_runtimes()
        .iter()
        .find(|manifest| manifest.id == "python:song")
        .expect("loaded")
        .clone();
    assert_eq!(manifest.companions[0].repo, "m-a-p/YuE2-Vae");

    shelf.approve("python:song").await;
    let (_, served) = shelf.booted().await;
    assert_eq!(served.runtime.id, Some(RuntimeId::from("python:song")));
    assert_eq!(served.state, ModelState::Ready);
    assert_eq!(served.modality, Modality::audio());
    assert_eq!(served.capabilities, vec![Capability::audio()]);
    assert_eq!(served.execution, ExecutionMode::Job);
    assert_eq!(keys(&served), ["lyrics", "cot", "seed"]);

    shelf.settings.revoke_runtime("python:song").unwrap();
    let (_, revoked) = shelf.booted().await;
    assert_eq!(revoked.runtime.id, None);
    assert!(revoked.capabilities.is_empty());
    assert!(
        revoked.params.is_empty(),
        "the lent schema goes with the runtime"
    );

    drop(shelf.root);
}

#[tokio::test]
async fn a_schema_dropped_from_the_manifest_leaves_the_model() {
    let shelf = Shelf::new("song", YUE2_MANIFEST, "yue2-3b");
    shelf.approve("python:song").await;
    assert_eq!(keys(&shelf.booted().await.1), ["lyrics", "cot", "seed"]);

    let manifest = runtimes_directory(&shelf.dirs)
        .join("song")
        .join("manifest.toml");
    let without = YUE2_MANIFEST.split("[[params]]").next().unwrap().to_owned()
        + "[serve]\nentrypoint = \"main.py\"\n\n[env]\nlockfile = \"requirements.lock\"\n";
    std::fs::write(&manifest, without).unwrap();
    shelf.approve("python:song").await;
    let (_, edited) = shelf.booted().await;
    assert_eq!(edited.runtime.id, Some(RuntimeId::from("python:song")));
    assert!(edited.params.is_empty(), "{:?}", keys(&edited));
    drop(shelf.root);
}

#[tokio::test]
async fn a_companion_on_the_shelf_is_found_and_counted() {
    let shelf = Shelf::new("song", YUE2_MANIFEST, "yue2-3b");
    shelf.approve("python:song").await;

    let (_, alone) = shelf.booted().await;
    assert_eq!(alone.companions.len(), 1);
    assert_eq!(alone.companions[0].repo, "m-a-p/YuE2-Vae");
    assert_eq!(alone.companions[0].path, None, "not pulled yet");
    assert_eq!(alone.missing_companions().count(), 1);
    let own = alone.serving_size().expect("measured");

    let snapshot = shelf.shelve_hub_repo("m-a-p/YuE2-Vae", 530_000_000);
    let (_, paired) = shelf.booted().await;
    assert_eq!(
        paired.companions[0].path.as_deref(),
        Some(snapshot.to_string_lossy().as_ref())
    );
    assert_eq!(paired.missing_companions().count(), 0);
    assert_eq!(paired.serving_size(), Some(own + 530_000_000));

    shelf.settings.revoke_runtime("python:song").unwrap();
    let (_, revoked) = shelf.booted().await;
    assert!(
        revoked.companions.is_empty(),
        "nothing needs it without the runtime"
    );
    assert_eq!(revoked.serving_size(), Some(own));
    drop(shelf.root);
}

#[tokio::test]
async fn a_lent_schema_is_not_replaced_by_the_generic_speech_profile() {
    // Orpheus identifies as a voice with no capability; a manifest that speaks
    // with its own parameters keeps them rather than the voice/speed profile
    // every `speak` model would otherwise get.
    let shelf = Shelf::new("voice", VOICE_MANIFEST, "orpheus-3b");
    shelf.approve("python:voice").await;
    let (_, served) = shelf.booted().await;
    assert_eq!(served.runtime.id, Some(RuntimeId::from("python:voice")));
    assert_eq!(served.modality, Modality::speech());
    assert_eq!(served.capabilities, vec![Capability::speak()]);
    assert_eq!(keys(&served), ["emotion"]);
    drop(shelf.root);
}

#[tokio::test]
async fn an_mlx_conversion_of_yue2_is_not_put_on_mlx_lm_as_a_chat_model() {
    let shelf = Shelf::new("song", YUE2_MANIFEST, "yue2-3b");
    let config = shelf.root.join("model").join("config.json");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        text.replacen('{', r#"{"quantization":{"bits":4,"group_size":64},"#, 1),
    )
    .unwrap();
    let (_, parked) = shelf.booted().await;
    assert_eq!(parked.runtime.id, None, "no built-in runtime takes it");
    assert_eq!(parked.modality, Modality::audio());
    assert!(!parked.capabilities.contains(&Capability::chat()));
    drop(shelf.root);
}

#[tokio::test]
async fn a_model_type_rule_leaves_the_decoder_alone() {
    let shelf = Shelf::new("song", YUE2_MANIFEST, "yue2-vae");
    shelf.approve("python:song").await;
    let (_, decoder) = shelf.booted().await;
    assert_eq!(decoder.runtime.id, None, "yue2_vae is not yue2");
    assert!(decoder.capabilities.is_empty());
    drop(shelf.root);
}

#[tokio::test]
async fn the_shipped_yue2_runtime_serves_yue2_once_approved() {
    let shelf = Shelf::new("", "", "yue2-3b");
    let (kernel, parked) = shelf.booted().await;
    assert_eq!(
        parked.runtime.id, None,
        "a shipped runtime waits for approval too"
    );
    let manifest = kernel
        .manifest_runtimes()
        .iter()
        .find(|manifest| manifest.id == "python:yue2")
        .expect("python:yue2 ships")
        .clone();
    assert_eq!(manifest.companions[0].repo, "m-a-p/YuE2-Vae");
    let lock = std::fs::read_to_string(
        manifest
            .directory
            .as_ref()
            .expect("a bundle directory")
            .join("requirements.lock"),
    )
    .unwrap();
    assert!(
        lock.contains("yue2_infer-0.1.5-py3-none-any.whl \\\n    --hash=sha256:"),
        "the package YuE2 runs is pinned by hash"
    );

    shelf.approve("python:yue2").await;
    let (_, served) = shelf.booted().await;
    assert_eq!(served.runtime.id, Some(RuntimeId::from("python:yue2")));
    assert_eq!(served.state, ModelState::Ready);
    assert_eq!(served.capabilities, vec![Capability::audio()]);
    assert_eq!(served.execution, ExecutionMode::Job);
    assert_eq!(keys(&served), ["lyrics", "cot", "abc", "cfg_scale", "seed"]);
    assert_eq!(
        served.missing_companions().count(),
        1,
        "the decoder is not pulled"
    );
    drop(shelf.root);
}

#[tokio::test]
async fn a_rule_that_also_matches_the_companion_does_not_make_it_its_own() {
    // A loose rule (any config.json) matches the decoder too; resolving it
    // again and again must not count it as its own companion.
    let loose = YUE2_MANIFEST.replace(
        r#"detect = { model_type = "yue2" }"#,
        r#"detect = { file = "config.json" }"#,
    );
    let shelf = Shelf::new("song", &loose, "yue2-3b");
    shelf.shelve_hub_repo("m-a-p/YuE2-Vae", 530_000_000);
    std::fs::write(
        shelf
            .root
            .join("hub/models--m-a-p--YuE2-Vae/snapshots/abc/config.json"),
        r#"{"model_type":"yue2_vae"}"#,
    )
    .unwrap();
    shelf.approve("python:song").await;
    let (kernel, _) = shelf.booted().await;
    kernel.resolve().await.expect("resolve again");
    let decoder = kernel
        .shelf()
        .await
        .iter()
        .find(|record| record.holds_hub_repo("m-a-p/YuE2-Vae"))
        .expect("the decoder is on the shelf")
        .clone();
    assert!(decoder.companions.is_empty(), "{:?}", decoder.companions);
    assert_eq!(decoder.serving_size(), Some(530_000_000));
    drop(shelf.root);
}
