//! Tests for the LM Studio scanner, driven by fake model trees under a temp dir.

mod support;

use std::path::{Path, PathBuf};

use kernel::discovery::{
    DiscoveredModel, LMStudioScanner, ScanResult, StoreScanner, discovered_models,
};
use kernel::records::{Capability, Modality, SourceKind};
use support::TempDir;

fn gguf(root: &Path, relative: &str, size: usize) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, vec![0u8; size]).unwrap();
}

fn find<'a>(result: &'a ScanResult, name: &str) -> &'a DiscoveredModel {
    result
        .discovered
        .iter()
        .find(|model| model.name == name)
        .unwrap_or_else(|| panic!("no model named {name}: {:?}", result.discovered))
}

#[test]
fn scans_a_loose_gguf_model() {
    let dir = TempDir::new();
    gguf(dir.path(), "TheBloke/Llama-3/weights.gguf", 2048);

    let result = LMStudioScanner::single(dir.path()).scan();
    assert!(result.issues.is_empty());
    let model = find(&result, "weights");
    assert_eq!(model.source.kind, SourceKind::lm_studio());
    assert_eq!(model.source.repo.as_deref(), Some("TheBloke/Llama-3"));
    assert_eq!(model.modality_hint, Some(Modality::text()));
    assert!(model.capabilities_hint.contains(&Capability::chat()));
    assert_eq!(model.footprint_bytes, 2048);
    assert!(
        model
            .primary_weight_path
            .as_deref()
            .unwrap()
            .ends_with("weights.gguf")
    );
    assert!(!model.downloading);
}

#[test]
fn groups_a_complete_shard_set_into_one_model() {
    let dir = TempDir::new();
    gguf(dir.path(), "org/big/llama-00001-of-00002.gguf", 100);
    gguf(dir.path(), "org/big/llama-00002-of-00002.gguf", 200);

    let result = LMStudioScanner::single(dir.path()).scan();
    let model = find(&result, "llama");
    assert_eq!(model.footprint_bytes, 300);
    assert!(!model.downloading);
    assert_eq!(model.source.repo.as_deref(), Some("org/big"));
    assert!(
        model
            .primary_weight_path
            .as_deref()
            .unwrap()
            .ends_with("llama-00001-of-00002.gguf")
    );
}

#[test]
fn an_incomplete_shard_set_is_downloading() {
    let dir = TempDir::new();
    // Only shard 1 of 2 is present (its first part exists, so it's still emitted).
    gguf(dir.path(), "org/big/m-00001-of-00002.gguf", 100);

    let result = LMStudioScanner::single(dir.path()).scan();
    assert!(find(&result, "m").downloading);
}

#[test]
fn a_shard_set_missing_its_first_part_is_an_issue() {
    let dir = TempDir::new();
    gguf(dir.path(), "org/big/m-00002-of-00002.gguf", 100);

    let result = LMStudioScanner::single(dir.path()).scan();
    assert!(result.discovered.is_empty());
    assert!(
        result
            .issues
            .iter()
            .any(|issue| issue.contains("missing its first part"))
    );
}

#[test]
fn skips_multimodal_projector_files() {
    let dir = TempDir::new();
    gguf(dir.path(), "org/vlm/model.gguf", 10);
    gguf(dir.path(), "org/vlm/mmproj-model.gguf", 500);

    let result = LMStudioScanner::single(dir.path()).scan();
    assert_eq!(result.discovered.len(), 1);
    assert_eq!(find(&result, "model").footprint_bytes, 10);
}

#[test]
fn repo_is_none_for_shallow_paths() {
    let dir = TempDir::new();
    // Two components deep (model/file) is < 3, so no repo.
    gguf(dir.path(), "solo/weights.gguf", 4);

    let result = LMStudioScanner::single(dir.path()).scan();
    assert_eq!(find(&result, "weights").source.repo, None);
}

#[test]
fn a_missing_root_is_silent() {
    let dir = TempDir::new();
    let result = LMStudioScanner::single(dir.path().join("gone")).scan();
    assert!(result.discovered.is_empty());
    assert!(result.failed_kinds.is_empty());
}

#[test]
fn a_root_that_is_a_file_fails_the_kind() {
    let dir = TempDir::new();
    let file = dir.path().join("models");
    std::fs::write(&file, b"x").unwrap();
    let result = LMStudioScanner::single(file).scan();
    assert_eq!(result.failed_kinds, vec![SourceKind::lm_studio()]);
}

#[test]
fn scans_across_multiple_roots() {
    let a = TempDir::new();
    let b = TempDir::new();
    gguf(a.path(), "org/one/first.gguf", 1);
    gguf(b.path(), "org/two/second.gguf", 1);

    let result = LMStudioScanner::new(vec![a.path().to_path_buf(), b.path().to_path_buf()]).scan();
    let mut names: Vec<&str> = result.discovered.iter().map(|m| m.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["first", "second"]);
}

#[test]
fn an_uppercase_extension_is_still_a_gguf() {
    let dir = TempDir::new();
    gguf(dir.path(), "org/model/WEIGHTS.GGUF", 8);

    let result = LMStudioScanner::single(dir.path()).scan();
    assert_eq!(find(&result, "WEIGHTS").footprint_bytes, 8);
}

#[test]
fn an_empty_but_existing_root_yields_nothing_without_failing() {
    let dir = TempDir::new();
    std::fs::create_dir_all(dir.path().join("org/model")).unwrap();
    std::fs::write(dir.path().join("org/model/README.md"), b"hi").unwrap();

    let result = LMStudioScanner::single(dir.path()).scan();
    assert!(result.discovered.is_empty());
    assert!(result.failed_kinds.is_empty());
    assert!(result.issues.is_empty());
}

#[test]
fn hidden_files_and_directories_are_skipped() {
    let dir = TempDir::new();
    gguf(dir.path(), "org/model/.secret.gguf", 4);
    gguf(dir.path(), "org/.hidden/inside.gguf", 4);
    gguf(dir.path(), "org/model/real.gguf", 4);

    let result = LMStudioScanner::single(dir.path()).scan();
    assert_eq!(result.discovered.len(), 1);
    assert_eq!(find(&result, "real").footprint_bytes, 4);
}

#[test]
fn an_mmproj_shard_is_filtered_before_grouping() {
    let dir = TempDir::new();
    gguf(dir.path(), "org/vlm/mmproj-model-00001-of-00002.gguf", 4);
    gguf(dir.path(), "org/vlm/model.gguf", 4);

    let result = LMStudioScanner::single(dir.path()).scan();
    // Only the real model — the mmproj shard is dropped before it can form (or
    // fail to form) a group, so no "missing first part" issue either.
    assert_eq!(result.discovered.len(), 1);
    assert_eq!(find(&result, "model").footprint_bytes, 4);
    assert!(result.issues.is_empty());
}

#[cfg(unix)]
#[test]
fn follows_a_symlinked_file_but_not_a_symlinked_directory() {
    let dir = TempDir::new();
    let real = TempDir::new();
    // A real weight elsewhere, symlinked into the scanned root as a file.
    gguf(real.path(), "w.gguf", 16);
    std::fs::create_dir_all(dir.path().join("org/model")).unwrap();
    std::os::unix::fs::symlink(
        real.path().join("w.gguf"),
        dir.path().join("org/model/linked.gguf"),
    )
    .unwrap();
    // A symlinked directory must NOT be recursed (loop safety); its contents
    // don't appear.
    gguf(real.path(), "deep/hidden.gguf", 1);
    std::os::unix::fs::symlink(real.path().join("deep"), dir.path().join("loop")).unwrap();

    let result = LMStudioScanner::single(dir.path()).scan();
    let names: Vec<&str> = result.discovered.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        ["linked"],
        "linked file followed, linked dir not recursed"
    );
    assert_eq!(find(&result, "linked").footprint_bytes, 16);
}

#[test]
fn discovered_models_uses_the_first_size_for_a_duplicated_path() {
    // The size map uniques on FIRST (Swift's uniquingKeysWith:{a,_ in a}); the
    // loose list itself isn't deduped, so both entries resolve to the first size.
    let path = PathBuf::from("/models/org/thing/w.gguf");
    let files = vec![(path.clone(), 100i64), (path, 999i64)];
    let (models, issues) = discovered_models(&files, &SourceKind::lm_studio(), |_| {
        Some("org/thing".to_owned())
    });
    assert!(issues.is_empty());
    assert!(
        models.iter().all(|model| model.footprint_bytes == 100),
        "first size wins: {models:?}"
    );
    assert!(
        models
            .iter()
            .all(|model| model.source.repo.as_deref() == Some("org/thing"))
    );
}
