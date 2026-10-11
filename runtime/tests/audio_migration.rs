//! A shelf written before audio generation was recognized (migration level 2)
//! is put right on its first read, without a rescan: YuE2 and Orpheus stop
//! claiming chat, MusicGen and AudioLDM2 gain their capability and runtime,
//! and a voice the Kokoro-only speech runtime cannot load leaves it. A real
//! chat model, Kokoro, a pinned record, one whose weights are gone and one
//! still downloading are left exactly as they were, and a later resolve
//! changes nothing. The model folders hold the real `config.json`,
//! `model_index.json` and card captured under `kernel/tests/fixtures/audio`.

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kernel::records::{
    Capability, Modality, ModelRecord, ModelSource, ModelState, Resolution, RuntimeId, SourceKind,
};
use kernel::registry::Registry;
use runtime::boot::{HedosDirs, build_kernel};
use runtime::settings::SettingsStore;
use support::TempDir;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../kernel/tests/fixtures/audio")
        .join(name)
}

/// A folder of `name`'s fixture files and a stub weight, registered the way
/// an older shelf held it.
fn register(
    root: &TempDir,
    registry: &mut Registry,
    name: &str,
    modality: Modality,
    capabilities: Vec<Capability>,
    runtime: Option<RuntimeId>,
) -> String {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    if fixture(name).is_dir() {
        for entry in std::fs::read_dir(fixture(name)).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
        }
    }
    std::fs::write(dir.join("model.safetensors"), b"weights").unwrap();
    let mut record = ModelRecord::new(
        name,
        modality,
        capabilities,
        ModelSource::new(SourceKind::folder(), &dir.to_string_lossy()),
    );
    record.primary_weight_path = Some(dir.join("model.safetensors").to_string_lossy().into_owned());
    record.state = if runtime.is_some() {
        ModelState::Ready
    } else {
        ModelState::Unresolved
    };
    record.runtime.id = runtime;
    let id = record.id.clone();
    registry.register(record).unwrap();
    id
}

fn on<'a>(shelf: &'a [ModelRecord], id: &str) -> &'a ModelRecord {
    shelf
        .iter()
        .find(|record| record.id == id)
        .expect("on the shelf")
}

#[tokio::test]
async fn an_older_shelfs_audio_models_are_put_right_on_first_sight() {
    let root = TempDir::new();
    let dirs = HedosDirs {
        data: root.join("data"),
    };
    let registry_dir = dirs.sub("registry");
    std::fs::create_dir_all(&registry_dir).unwrap();
    let mut registry = Registry::open(&registry_dir).unwrap();
    let chat = || vec![Capability::chat(), Capability::complete()];
    let yue2 = register(
        &root,
        &mut registry,
        "yue2-3b",
        Modality::text(),
        chat(),
        None,
    );
    let orpheus = register(
        &root,
        &mut registry,
        "orpheus-3b",
        Modality::text(),
        chat(),
        None,
    );
    let musicgen = register(
        &root,
        &mut registry,
        "musicgen-small",
        Modality::unknown(),
        Vec::new(),
        None,
    );
    let bark = register(
        &root,
        &mut registry,
        "bark",
        Modality::speech(),
        vec![Capability::speak()],
        Some(RuntimeId::mlx_audio()),
    );
    let kokoro = register(
        &root,
        &mut registry,
        "kokoro-82m-bf16",
        Modality::speech(),
        vec![Capability::speak()],
        Some(RuntimeId::mlx_audio()),
    );
    let llama_dir = root.join("llama");
    std::fs::create_dir_all(&llama_dir).unwrap();
    std::fs::write(
        llama_dir.join("config.json"),
        r#"{"architectures":["LlamaForCausalLM"],"model_type":"llama"}"#,
    )
    .unwrap();
    let llama = register(
        &root,
        &mut registry,
        "llama",
        Modality::text(),
        chat(),
        None,
    );
    let audioldm = register(
        &root,
        &mut registry,
        "audioldm2-music",
        Modality::audio(),
        Vec::new(),
        None,
    );

    // Records the pass must leave alone whatever their files now say.
    let pinned = register(
        &root,
        &mut registry,
        "yue-s1-7b",
        Modality::text(),
        chat(),
        Some(RuntimeId::mlx_lm()),
    );
    let missing = register(
        &root,
        &mut registry,
        "heartmula-3b",
        Modality::text(),
        chat(),
        None,
    );
    let downloading = register(
        &root,
        &mut registry,
        "ace-step-1.5",
        Modality::text(),
        chat(),
        None,
    );
    let mut adjusted = registry.records();
    let record = adjusted.get_mut(&pinned).unwrap();
    record.runtime.resolved = Resolution::User;
    let record = adjusted.get_mut(&missing).unwrap();
    record.state = ModelState::Missing;
    let record = adjusted.get_mut(&downloading).unwrap();
    record.downloading = true;
    for record in adjusted.values() {
        registry.register(record.clone()).unwrap();
    }
    let before: BTreeMap<String, ModelRecord> = registry.records();
    assert!(
        registry.mark_migrated(2, &before).unwrap(),
        "an older shelf"
    );
    drop(registry);

    let settings = SettingsStore::new(root.join("hedos.toml"));
    let kernel = build_kernel(&dirs, &settings.load()).expect("build kernel");
    let shelf = kernel.shelf().await;

    let yue2 = on(&shelf, &yue2);
    assert_eq!(yue2.modality, Modality::audio());
    assert!(yue2.capabilities.is_empty(), "{:?}", yue2.capabilities);

    let orpheus = on(&shelf, &orpheus);
    assert_eq!(orpheus.modality, Modality::speech());
    assert!(orpheus.capabilities.is_empty());

    for id in [&musicgen, &audioldm] {
        let generator = on(&shelf, id);
        assert_eq!(generator.capabilities, vec![Capability::audio()], "{id}");
        assert_eq!(generator.runtime.id, Some(RuntimeId::audio()), "{id}");
        assert_eq!(generator.state, ModelState::Ready, "{id}");
    }

    let bark = on(&shelf, &bark);
    assert_eq!(bark.runtime.id, None, "the Kokoro runtime cannot load Bark");
    assert_eq!(bark.state, ModelState::Unresolved);

    let kokoro = on(&shelf, &kokoro);
    assert_eq!(kokoro.runtime.id, Some(RuntimeId::mlx_audio()));
    assert_eq!(kokoro.capabilities, vec![Capability::speak()]);

    let llama = on(&shelf, &llama);
    assert!(
        llama.capabilities.contains(&Capability::chat()),
        "a chat model is left alone"
    );

    // Left as they were by this pass (the tool fold every shelf read runs may
    // still add `tools` to a chat model on mlx-lm).
    for id in [&pinned, &missing, &downloading] {
        let (now, then) = (on(&shelf, id), &before[id]);
        assert_eq!(now.modality, then.modality, "{id}");
        assert_eq!(now.runtime, then.runtime, "{id}");
        assert_eq!(now.state, then.state, "{id}");
        assert!(now.capabilities.contains(&Capability::chat()), "{id}");
    }

    assert_eq!(Registry::open(&registry_dir).unwrap().migration_level(), 3);

    // A full resolve afterwards agrees with what the migration left.
    kernel.resolve().await.expect("resolve");
    let again = kernel.shelf().await;
    for id in [
        &yue2.id,
        &orpheus.id,
        &musicgen,
        &audioldm,
        &bark.id,
        &kokoro.id,
    ] {
        let (now, then) = (on(&again, id), on(&shelf, id));
        assert_eq!(
            (&now.modality, &now.capabilities, &now.runtime.id),
            (&then.modality, &then.capabilities, &then.runtime.id),
            "{id}"
        );
    }
}
