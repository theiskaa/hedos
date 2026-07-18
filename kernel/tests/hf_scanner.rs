//! Tests for the Hugging Face cache scanner, driven by fake cache trees under a
//! temp dir. (Snapshot files are real, not symlinks, for portability — the
//! scanner reads them the same way.)

mod support;

use std::path::{Path, PathBuf};

use kernel::discovery::{DiscoveredModel, HFCacheScanner, ScanResult, StoreScanner};
use kernel::records::{Capability, ExecutionMode, Modality, SourceKind};
use support::TempDir;

fn model_dir(root: &Path, org: &str, name: &str) -> PathBuf {
    let dir = root.join(format!("models--{org}--{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn snapshot_dir(repo: &Path, revision: &str) -> PathBuf {
    let snapshot = repo.join("snapshots").join(revision);
    std::fs::create_dir_all(&snapshot).unwrap();
    snapshot
}

fn write(path: &Path, contents: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn refs_main(repo: &Path, revision: &str) {
    write(&repo.join("refs").join("main"), revision.as_bytes());
}

fn blob(repo: &Path, name: &str, size: usize) {
    write(&repo.join("blobs").join(name), &vec![0u8; size]);
}

fn find<'a>(result: &'a ScanResult, name: &str) -> &'a DiscoveredModel {
    result
        .discovered
        .iter()
        .find(|model| model.name == name)
        .unwrap_or_else(|| panic!("no model named {name}: {:?}", result.discovered))
}

/// A standard single-repo cache: `refs/main`, one snapshot with a config.json +
/// weight + tokenizer, and a blob for the footprint.
fn standard_repo(root: &Path) -> PathBuf {
    let repo = model_dir(root, "meta", "Llama-3");
    refs_main(&repo, "abc123");
    let snapshot = snapshot_dir(&repo, "abc123");
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"],"max_position_embeddings":8192}"#,
    );
    write(&snapshot.join("model.safetensors"), &[0u8; 10]);
    write(&snapshot.join("tokenizer.json"), b"{}");
    blob(&repo, "weight-blob", 4096);
    repo
}

#[test]
fn scans_a_config_json_model_with_provenance() {
    let dir = TempDir::new();
    standard_repo(dir.path());

    let result = HFCacheScanner::single(dir.path()).scan();
    assert!(result.failed_kinds.is_empty());
    let model = find(&result, "Llama-3");

    assert_eq!(model.source.kind, SourceKind::huggingface_cache());
    assert_eq!(model.source.repo.as_deref(), Some("meta/Llama-3"));
    assert_eq!(model.source.reference.as_deref(), Some("abc123"));
    assert_eq!(model.modality_hint, Some(Modality::text()));
    assert!(model.capabilities_hint.contains(&Capability::chat()));
    assert_eq!(model.context_length_hint, Some(8192));
    assert_eq!(model.footprint_bytes, 4096);
    assert!(
        model
            .primary_weight_path
            .as_deref()
            .unwrap()
            .ends_with("model.safetensors")
    );
    assert!(
        model.diagnostics.is_empty(),
        "no diagnostics: {:?}",
        model.diagnostics
    );
    assert!(!model.downloading);
}

#[test]
fn falls_back_to_the_only_snapshot_without_refs_main() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "model");
    let snapshot = snapshot_dir(&repo, "rev9");
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["MistralForCausalLM"]}"#,
    );
    write(&snapshot.join("tokenizer.model"), b"x");

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "model");
    assert_eq!(model.source.reference.as_deref(), Some("rev9"));
}

#[test]
fn a_repo_without_a_usable_snapshot_is_an_issue() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "empty");
    std::fs::create_dir_all(repo.join("snapshots")).unwrap();

    let result = HFCacheScanner::single(dir.path()).scan();
    assert!(result.discovered.is_empty());
    assert!(
        result
            .issues
            .iter()
            .any(|issue| issue.contains("no usable snapshot"))
    );
}

#[test]
fn a_bare_gguf_snapshot_gets_the_gguf_hint() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "ggufonly");
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    write(&snapshot.join("model.gguf"), b"GGUF-ish");

    let model_result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&model_result, "ggufonly");
    assert_eq!(model.modality_hint, Some(Modality::text()));
    assert!(model.capabilities_hint.contains(&Capability::chat()));
}

#[test]
fn a_snapshot_without_config_gets_a_diagnostic() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "mystery");
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    write(&snapshot.join("README.md"), b"hi");

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "mystery");
    assert!(
        model
            .diagnostics
            .iter()
            .any(|note| note.contains("no config.json or model_index.json"))
    );
    assert_eq!(model.primary_weight_path, None);
}

#[test]
fn a_text_model_missing_a_tokenizer_is_flagged() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "notok");
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    );

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "notok");
    assert!(
        model
            .diagnostics
            .iter()
            .any(|note| note.contains("no tokenizer"))
    );
}

#[test]
fn sentence_transformers_markers_override_to_embedding() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "st");
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    // A text architecture (so the context length is captured) that the
    // sentence-transformers marker then overrides to an embedding model.
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"],"max_position_embeddings":512}"#,
    );
    write(&snapshot.join("config_sentence_transformers.json"), b"{}");
    write(&snapshot.join("tokenizer.json"), b"{}");

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "st");
    assert_eq!(model.modality_hint, Some(Modality::embedding()));
    assert_eq!(model.capabilities_hint, vec![Capability::embed()]);
    // The context length carries over from the original config hint.
    assert_eq!(model.context_length_hint, Some(512));
}

#[test]
fn a_model_index_snapshot_is_a_job() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "diffusion");
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    write(
        &snapshot.join("model_index.json"),
        br#"{"_class_name":"X"}"#,
    );

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "diffusion");
    assert_eq!(model.execution_hint, ExecutionMode::Job);
    assert_eq!(model.modality_hint, None);
}

#[test]
fn an_incomplete_blob_marks_the_model_downloading() {
    let dir = TempDir::new();
    let repo = standard_repo(dir.path());
    write(&repo.join("blobs").join("half.incomplete"), b"partial");

    let result = HFCacheScanner::single(dir.path()).scan();
    assert!(find(&result, "Llama-3").downloading);
}

#[test]
fn a_missing_index_shard_marks_the_model_downloading() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "sharded");
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    );
    write(&snapshot.join("tokenizer.json"), b"{}");
    // Only shard 1 of 2 is present on disk.
    write(
        &snapshot.join("model-00001-of-00002.safetensors"),
        &[0u8; 4],
    );
    write(
        &snapshot.join("model.safetensors.index.json"),
        br#"{"weight_map":{"a":"model-00001-of-00002.safetensors","b":"model-00002-of-00002.safetensors"}}"#,
    );

    let result = HFCacheScanner::single(dir.path()).scan();
    assert!(find(&result, "sharded").downloading);
}

#[test]
fn incomplete_gguf_shards_mark_the_model_downloading() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "ggufshard");
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    // Shard 1 of 2, but not shard 2.
    write(&snapshot.join("model-00001-of-00002.gguf"), b"x");

    let result = HFCacheScanner::single(dir.path()).scan();
    assert!(find(&result, "ggufshard").downloading);
}

#[test]
fn an_mmproj_file_is_not_the_primary_weight() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "vlm");
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    );
    write(&snapshot.join("tokenizer.json"), b"{}");
    write(&snapshot.join("mmproj-model.safetensors"), &[0u8; 100]);
    write(&snapshot.join("model.safetensors"), &[0u8; 10]);

    let result = HFCacheScanner::single(dir.path()).scan();
    let weight = find(&result, "vlm").primary_weight_path.clone().unwrap();
    assert!(weight.ends_with("model.safetensors"));
    assert!(!weight.contains("mmproj"));
}

#[test]
fn a_required_user_root_that_is_missing_fails_the_kind() {
    let dir = TempDir::new();
    let scanner = HFCacheScanner::with_user_roots(vec![], vec![dir.path().join("gone")]);
    let result = scanner.scan();
    assert_eq!(result.failed_kinds, vec![SourceKind::huggingface_cache()]);
}

#[test]
fn a_missing_optional_root_is_silent() {
    let dir = TempDir::new();
    let result = HFCacheScanner::single(dir.path().join("gone")).scan();
    assert!(result.discovered.is_empty());
    assert!(result.failed_kinds.is_empty());
}

#[test]
fn ignores_directories_that_are_not_model_repos() {
    let dir = TempDir::new();
    std::fs::create_dir_all(dir.path().join("version.txt")).unwrap();
    standard_repo(dir.path());

    let result = HFCacheScanner::single(dir.path()).scan();
    assert_eq!(result.discovered.len(), 1);
}

#[test]
fn a_refs_main_pointing_at_a_missing_snapshot_falls_back() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "stale");
    // refs/main names a snapshot that isn't on disk; the one present snapshot wins.
    refs_main(&repo, "deleted");
    let snapshot = snapshot_dir(&repo, "present");
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    );
    write(&snapshot.join("tokenizer.json"), b"{}");

    let result = HFCacheScanner::single(dir.path()).scan();
    assert_eq!(
        find(&result, "stale").source.reference.as_deref(),
        Some("present")
    );
}

#[test]
fn a_non_object_weight_map_does_not_flag_downloading() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "weird-index");
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    );
    write(&snapshot.join("tokenizer.json"), b"{}");
    // weight_map is an array, not an object — safely ignored.
    write(
        &snapshot.join("model.safetensors.index.json"),
        br#"{"weight_map":[1,2,3]}"#,
    );

    let result = HFCacheScanner::single(dir.path()).scan();
    assert!(!find(&result, "weird-index").downloading);
}

#[test]
fn a_pooling_directory_marker_overrides_to_embedding() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "pooled");
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    );
    write(&snapshot.join("tokenizer.json"), b"{}");
    // The second sentence-transformers marker is a directory.
    std::fs::create_dir_all(snapshot.join("1_Pooling")).unwrap();

    let result = HFCacheScanner::single(dir.path()).scan();
    assert_eq!(
        find(&result, "pooled").modality_hint,
        Some(Modality::embedding())
    );
}

#[test]
fn a_bin_weight_is_primary_only_with_ggml_magic() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "ggmlbin");
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    );
    write(&snapshot.join("tokenizer.json"), b"{}");
    // `lmgg` is the legacy GGML magic; a plain .bin without it is not a weight.
    write(&snapshot.join("model.bin"), b"lmggDATA");

    let result = HFCacheScanner::single(dir.path()).scan();
    let weight = find(&result, "ggmlbin")
        .primary_weight_path
        .clone()
        .unwrap();
    assert!(
        weight.ends_with("model.bin"),
        "ggml .bin is the weight: {weight}"
    );

    // Now a non-magic .bin: no weight file at all.
    let dir2 = TempDir::new();
    let repo2 = model_dir(dir2.path(), "org", "plainbin");
    refs_main(&repo2, "r1");
    let snap2 = snapshot_dir(&repo2, "r1");
    write(
        &snap2.join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    );
    write(&snap2.join("tokenizer.json"), b"{}");
    write(&snap2.join("weights.bin"), b"not-magic");
    let result2 = HFCacheScanner::single(dir2.path()).scan();
    assert_eq!(find(&result2, "plainbin").primary_weight_path, None);
}

#[test]
fn discovers_multiple_repos_in_one_root() {
    let dir = TempDir::new();
    for name in ["A", "B"] {
        let repo = model_dir(dir.path(), "org", name);
        refs_main(&repo, "r1");
        let snapshot = snapshot_dir(&repo, "r1");
        write(&snapshot.join("model.gguf"), b"x");
    }
    let mut names: Vec<String> = HFCacheScanner::single(dir.path())
        .scan()
        .discovered
        .into_iter()
        .map(|model| model.name)
        .collect();
    names.sort();
    assert_eq!(names, ["A", "B"]);
}
