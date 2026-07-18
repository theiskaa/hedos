//! Tests for `ModelHabitat`: root resolution from env/home/settings and the
//! scanner assembly with kind filtering.

mod support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use kernel::discovery::{ModelHabitat, ModelsSettings};
use kernel::records::SourceKind;
use support::TempDir;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn roots_of(habitat: &ModelHabitat, settings: &ModelsSettings) -> Vec<(SourceKind, PathBuf)> {
    habitat.roots(settings)
}

fn contains(roots: &[(SourceKind, PathBuf)], kind: &SourceKind, suffix: &str) -> bool {
    roots
        .iter()
        .any(|(k, path)| k == kind && path.to_string_lossy().ends_with(suffix))
}

#[test]
fn default_roots_cover_every_store_under_home() {
    let home = Path::new("/home/user");
    let habitat = ModelHabitat::new(home, HashMap::new());
    let roots = roots_of(&habitat, &ModelsSettings::default());

    assert!(contains(
        &roots,
        &SourceKind::ollama(),
        "/home/user/.ollama/models"
    ));
    assert!(contains(
        &roots,
        &SourceKind::huggingface_cache(),
        "/home/user/.cache/huggingface/hub"
    ));
    assert!(contains(
        &roots,
        &SourceKind::lm_studio(),
        "/home/user/.lmstudio/models"
    ));
    assert!(contains(
        &roots,
        &SourceKind::lm_studio(),
        "/home/user/.cache/lm-studio/models"
    ));
    assert!(contains(
        &roots,
        &SourceKind::file(),
        "/home/user/Downloads"
    ));
    assert!(contains(&roots, &SourceKind::file(), "/home/user/Models"));
}

#[test]
fn ollama_models_env_overrides_the_default_with_tilde_expansion() {
    let home = Path::new("/home/user");
    let habitat = ModelHabitat::new(home, env(&[("OLLAMA_MODELS", "~/custom/ollama")]));
    let roots = roots_of(&habitat, &ModelsSettings::default());
    assert!(contains(
        &roots,
        &SourceKind::ollama(),
        "/home/user/custom/ollama"
    ));
    // The default is not also present.
    assert!(!contains(&roots, &SourceKind::ollama(), ".ollama/models"));
}

#[test]
fn hf_env_roots_are_added_and_deduplicated() {
    let home = Path::new("/home/user");
    let habitat = ModelHabitat::new(
        home,
        env(&[("HF_HUB_CACHE", "/data/hub"), ("HF_HOME", "/data/hf")]),
    );
    let roots = roots_of(&habitat, &ModelsSettings::default());
    assert!(contains(
        &roots,
        &SourceKind::huggingface_cache(),
        "/data/hub"
    ));
    // HF_HOME contributes `<HF_HOME>/hub`.
    assert!(contains(
        &roots,
        &SourceKind::huggingface_cache(),
        "/data/hf/hub"
    ));

    // An HF_HUB_CACHE equal to the default cache path is not duplicated.
    let dup = ModelHabitat::new(
        home,
        env(&[("HF_HUB_CACHE", "/home/user/.cache/huggingface/hub")]),
    );
    let hf_count = roots_of(&dup, &ModelsSettings::default())
        .iter()
        .filter(|(k, path)| {
            k == &SourceKind::huggingface_cache() && path.ends_with(".cache/huggingface/hub")
        })
        .count();
    assert_eq!(hf_count, 1, "the default hub path appears once");
}

#[test]
fn watched_folders_become_file_roots() {
    let home = Path::new("/home/user");
    let habitat = ModelHabitat::new(home, HashMap::new());
    let settings = ModelsSettings {
        watched_folders: vec!["/mnt/models".to_owned()],
        hf_cache_roots: Vec::new(),
    };
    let roots = roots_of(&habitat, &settings);
    assert!(contains(&roots, &SourceKind::file(), "/mnt/models"));
}

#[test]
fn a_user_hf_root_prefers_an_existing_hub_subdirectory() {
    // Real dirs so `is_hub_directory` can see them.
    let dir = TempDir::new();
    let base = dir.path().join("hf");
    std::fs::create_dir_all(base.join("hub")).unwrap();

    let habitat = ModelHabitat::new("/home/user", HashMap::new());
    let settings = ModelsSettings {
        watched_folders: Vec::new(),
        hf_cache_roots: vec![base.to_string_lossy().into_owned()],
    };
    let roots = habitat.roots(&settings);
    // The `hub` subdir exists, so it is used (not the bare base).
    assert!(
        roots
            .iter()
            .any(|(k, path)| k == &SourceKind::huggingface_cache() && path == &base.join("hub"))
    );
}

#[test]
fn a_user_hf_root_without_a_hub_subdir_uses_the_bare_path() {
    let dir = TempDir::new();
    let base = dir.path().join("plainhf");
    std::fs::create_dir_all(&base).unwrap();

    let habitat = ModelHabitat::new("/home/user", HashMap::new());
    let settings = ModelsSettings {
        watched_folders: Vec::new(),
        hf_cache_roots: vec![base.to_string_lossy().into_owned()],
    };
    let roots = habitat.roots(&settings);
    assert!(
        roots
            .iter()
            .any(|(k, path)| k == &SourceKind::huggingface_cache() && path == &base)
    );
}

#[test]
fn scanners_includes_all_four_by_default() {
    let habitat = ModelHabitat::new("/home/user", HashMap::new());
    let scanners = habitat.scanners(None, &ModelsSettings::default());
    // One scanner per store: ollama, hf-cache, lm-studio, loose(file+folder).
    assert_eq!(scanners.len(), 4);

    let kinds: Vec<SourceKind> = scanners.iter().flat_map(|s| s.kinds()).collect();
    assert!(kinds.contains(&SourceKind::ollama()));
    assert!(kinds.contains(&SourceKind::huggingface_cache()));
    assert!(kinds.contains(&SourceKind::lm_studio()));
    assert!(kinds.contains(&SourceKind::file()));
    assert!(kinds.contains(&SourceKind::folder()));
}

#[test]
fn the_assembled_hf_scanner_does_not_scan_a_user_root_twice() {
    // Build a real HF cache under a user root and confirm each repo is discovered
    // exactly once (the user root must not be both a default and a user root).
    let dir = TempDir::new();
    let hub = dir.path().join("hub");
    let repo = hub.join("models--org--model");
    std::fs::create_dir_all(repo.join("snapshots").join("r1")).unwrap();
    std::fs::create_dir_all(repo.join("refs")).unwrap();
    std::fs::write(repo.join("refs").join("main"), b"r1").unwrap();
    std::fs::write(repo.join("snapshots").join("r1").join("model.gguf"), b"x").unwrap();

    let habitat = ModelHabitat::new(dir.path().join("nohome"), HashMap::new());
    let settings = ModelsSettings {
        watched_folders: Vec::new(),
        hf_cache_roots: vec![dir.path().to_string_lossy().into_owned()],
    };
    let scanners = habitat.scanners(Some(&[SourceKind::huggingface_cache()]), &settings);
    assert_eq!(scanners.len(), 1);
    let discovered = scanners[0].scan().discovered;
    assert_eq!(
        discovered.len(),
        1,
        "each repo discovered once: {discovered:?}"
    );
    assert_eq!(discovered[0].name, "model");
}

#[test]
fn hf_home_alone_contributes_its_hub() {
    let habitat = ModelHabitat::new("/home/user", env(&[("HF_HOME", "/data/hf")]));
    let roots = roots_of(&habitat, &ModelsSettings::default());
    assert!(contains(
        &roots,
        &SourceKind::huggingface_cache(),
        "/data/hf/hub"
    ));
}

#[test]
fn empty_env_values_are_ignored() {
    let habitat = ModelHabitat::new(
        "/home/user",
        env(&[("OLLAMA_MODELS", ""), ("HF_HUB_CACHE", ""), ("HF_HOME", "")]),
    );
    let roots = roots_of(&habitat, &ModelsSettings::default());
    // OLLAMA_MODELS="" falls back to the default.
    assert!(contains(
        &roots,
        &SourceKind::ollama(),
        "/home/user/.ollama/models"
    ));
    // The empty HF vars contribute nothing beyond the default cache.
    let hf: Vec<&PathBuf> = roots
        .iter()
        .filter(|(k, _)| k == &SourceKind::huggingface_cache())
        .map(|(_, path)| path)
        .collect();
    assert_eq!(hf.len(), 1);
    assert!(hf[0].ends_with(".cache/huggingface/hub"));
}

#[test]
fn a_tilde_only_ollama_path_expands_to_home() {
    let habitat = ModelHabitat::new("/home/user", env(&[("OLLAMA_MODELS", "~")]));
    let roots = roots_of(&habitat, &ModelsSettings::default());
    assert!(
        roots
            .iter()
            .any(|(k, path)| k == &SourceKind::ollama() && path == Path::new("/home/user"))
    );
}

#[test]
fn roots_are_ordered_ollama_first_and_watched_last() {
    let habitat = ModelHabitat::new("/home/user", HashMap::new());
    let settings = ModelsSettings {
        watched_folders: vec!["/mnt/w".to_owned()],
        hf_cache_roots: Vec::new(),
    };
    let roots = roots_of(&habitat, &settings);
    assert_eq!(roots.first().unwrap().0, SourceKind::ollama());
    let last = roots.last().unwrap();
    assert_eq!(last.0, SourceKind::file());
    assert_eq!(last.1, PathBuf::from("/mnt/w"));
}

#[test]
fn scanners_filters_by_requested_kind() {
    let habitat = ModelHabitat::new("/home/user", HashMap::new());

    let only_ollama = habitat.scanners(Some(&[SourceKind::ollama()]), &ModelsSettings::default());
    assert_eq!(only_ollama.len(), 1);
    assert!(only_ollama[0].kinds().contains(&SourceKind::ollama()));

    // `folder` selects the loose-file scanner (which produces file + folder).
    let only_loose = habitat.scanners(Some(&[SourceKind::folder()]), &ModelsSettings::default());
    assert_eq!(only_loose.len(), 1);
    assert!(only_loose[0].kinds().contains(&SourceKind::folder()));

    // A kind no scanner produces yields nothing.
    let none = habitat.scanners(Some(&[SourceKind::endpoint()]), &ModelsSettings::default());
    assert!(none.is_empty());
}
