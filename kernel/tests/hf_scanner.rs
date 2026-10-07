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
fn a_cross_encoder_is_a_reranker_that_claims_no_capability() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "rerank");
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    // A causal LM read at one logit per pair: neither a chat model, which its
    // config alone would say, nor the embedder its sentence-transformers file
    // would otherwise mark it as.
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["Qwen3ForCausalLM"],"max_position_embeddings":40960}"#,
    );
    write(
        &snapshot.join("config_sentence_transformers.json"),
        br#"{"model_type":"CrossEncoder"}"#,
    );
    write(&snapshot.join("tokenizer.json"), b"{}");

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "rerank");
    assert_eq!(model.modality_hint, Some(Modality::text()));
    assert!(model.capabilities_hint.is_empty());
    assert_eq!(model.context_length_hint, Some(40960));
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
fn a_repo_keeping_a_directory_per_quantization_still_has_weights() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "quantized");
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    // Nothing at the snapshot root: every weight is a level down, which is how
    // a repo shipping several quantizations lays them out.
    write(&snapshot.join("Q4_K_M").join("model.gguf"), b"GGUF");
    write(
        &snapshot.join("Q8_0").join("model.gguf"),
        &b"GGUF".repeat(50),
    );

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "quantized");
    assert_eq!(model.modality_hint, Some(Modality::text()));
    assert!(!model.downloading);
    assert!(
        model
            .primary_weight_path
            .as_deref()
            .is_some_and(|path| path.ends_with("Q8_0/model.gguf")),
        "the largest weight, wherever it sits: {:?}",
        model.primary_weight_path
    );
    assert!(
        !model
            .diagnostics
            .iter()
            .any(|line| line.contains("no config.json")),
        "weights below the root are still weights: {:?}",
        model.diagnostics
    );
}

#[test]
fn incomplete_gguf_shards_below_the_snapshot_mark_the_model_downloading() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "deepshard");
    refs_main(&repo, "r1");
    // Shard 1 of 2 inside a quantization directory, but not shard 2.
    write(
        &snapshot_dir(&repo, "r1")
            .join("Q4_K_M")
            .join("model-00001-of-00002.gguf"),
        b"GGUF",
    );

    let result = HFCacheScanner::single(dir.path()).scan();
    assert!(find(&result, "deepshard").downloading);
}

#[test]
fn a_directory_named_like_a_weight_is_not_the_primary_weight() {
    let dir = TempDir::new();
    let repo = model_dir(dir.path(), "org", "trap");
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    // A directory wearing a weight's name, holding the real one.
    write(&snapshot.join("model.gguf").join("real.gguf"), b"GGUF");

    let result = HFCacheScanner::single(dir.path()).scan();
    assert!(
        find(&result, "trap")
            .primary_weight_path
            .as_deref()
            .is_some_and(|path| path.ends_with("model.gguf/real.gguf")),
        "a server is handed a file, never a directory"
    );
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

/// Write a `size`-byte blob named `blob_name` and link `relative` in the
/// snapshot to it, the way the hub cache lays a file out.
fn linked(repo: &Path, snapshot: &Path, relative: &str, blob_name: &str, size: usize) {
    blob(repo, blob_name, size);
    let link = snapshot.join(relative);
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(repo.join("blobs").join(blob_name), link).unwrap();
}

fn hub_repo(root: &Path, name: &str) -> (PathBuf, PathBuf) {
    let repo = model_dir(root, "org", name);
    refs_main(&repo, "r1");
    let snapshot = snapshot_dir(&repo, "r1");
    (repo, snapshot)
}

#[test]
fn a_repo_with_several_quantizations_serves_from_one() {
    let dir = TempDir::new();
    let (repo, snapshot) = hub_repo(dir.path(), "multi");
    linked(&repo, &snapshot, "a.Q4_K_M.gguf", "q4", 100);
    linked(&repo, &snapshot, "a.Q8_0.gguf", "q8", 200);

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "multi");
    assert_eq!(model.footprint_bytes, 300);
    assert_eq!(model.serving_bytes, Some(200));
}

#[test]
fn a_directory_per_quantization_serves_the_primary_shard_set() {
    let dir = TempDir::new();
    let (repo, snapshot) = hub_repo(dir.path(), "sharded");
    linked(&repo, &snapshot, "Q4/m-00001-of-00002.gguf", "q4a", 50);
    linked(&repo, &snapshot, "Q4/m-00002-of-00002.gguf", "q4b", 50);
    linked(&repo, &snapshot, "Q8/m-00001-of-00002.gguf", "q8a", 90);
    linked(&repo, &snapshot, "Q8/m-00002-of-00002.gguf", "q8b", 80);

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "sharded");
    assert_eq!(model.footprint_bytes, 270);
    assert_eq!(model.serving_bytes, Some(170));
}

#[test]
fn a_projector_beside_the_weight_counts_once() {
    let dir = TempDir::new();
    let (repo, snapshot) = hub_repo(dir.path(), "vision");
    linked(&repo, &snapshot, "a.Q4_K_M.gguf", "weight", 100);
    linked(&repo, &snapshot, "mmproj-f16.gguf", "f16", 10);
    linked(&repo, &snapshot, "mmproj-f32.gguf", "f32", 20);

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "vision");
    assert_eq!(model.footprint_bytes, 130);
    assert_eq!(model.serving_bytes, Some(120));
}

#[test]
fn a_root_projector_counts_for_a_quant_directory() {
    let dir = TempDir::new();
    let (repo, snapshot) = hub_repo(dir.path(), "rooted");
    linked(&repo, &snapshot, "mmproj-f16.gguf", "projector", 30);
    linked(&repo, &snapshot, "Q8_0/a.Q8_0.gguf", "q8", 100);
    linked(&repo, &snapshot, "Q4_K_M/a.Q4_K_M.gguf", "q4", 60);

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "rooted");
    assert_eq!(model.serving_bytes, Some(130));
}

#[test]
fn an_indexed_safetensors_set_serves_every_shard() {
    let index = br#"{"weight_map":{"a":"model-00001-of-00002.safetensors","b":"model-00002-of-00002.safetensors"}}"#;
    let config = br#"{"architectures":["LlamaForCausalLM"]}"#;

    let dir = TempDir::new();
    let (repo, snapshot) = hub_repo(dir.path(), "indexed");
    linked(
        &repo,
        &snapshot,
        "model-00001-of-00002.safetensors",
        "s1",
        60,
    );
    linked(
        &repo,
        &snapshot,
        "model-00002-of-00002.safetensors",
        "s2",
        40,
    );
    write(&snapshot.join("model.safetensors.index.json"), index);
    write(&snapshot.join("config.json"), config);

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "indexed");
    assert_eq!(model.serving_bytes, Some(100 + config.len() as i64));

    let (repo, snapshot) = hub_repo(dir.path(), "consolidated");
    linked(
        &repo,
        &snapshot,
        "model-00001-of-00002.safetensors",
        "s1",
        60,
    );
    linked(
        &repo,
        &snapshot,
        "model-00002-of-00002.safetensors",
        "s2",
        40,
    );
    linked(&repo, &snapshot, "consolidated.safetensors", "whole", 150);
    write(&snapshot.join("model.safetensors.index.json"), index);
    write(&snapshot.join("config.json"), config);

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "consolidated");
    assert!(
        model
            .primary_weight_path
            .as_deref()
            .is_some_and(|path| path.ends_with("whole"))
    );
    assert_eq!(model.serving_bytes, Some(150 + config.len() as i64));
}

#[test]
fn support_files_beside_the_weight_count_and_docs_do_not() {
    let dir = TempDir::new();
    let (repo, snapshot) = hub_repo(dir.path(), "supported");
    linked(&repo, &snapshot, "a.Q4_K_M.gguf", "weight", 100);
    linked(&repo, &snapshot, "config.json", "config", 7);
    linked(&repo, &snapshot, "tokenizer.json", "tokenizer", 5);
    // A second name for one blob is one file loaded, not two.
    std::os::unix::fs::symlink(
        repo.join("blobs").join("tokenizer"),
        snapshot.join("tokenizer_copy.json"),
    )
    .unwrap();
    linked(&repo, &snapshot, "README.md", "readme", 50);
    linked(&repo, &snapshot, "diagram.png", "picture", 40);

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "supported");
    assert_eq!(model.footprint_bytes, 202);
    assert_eq!(model.serving_bytes, Some(112));
}

#[test]
fn a_snapshot_without_a_primary_has_no_serving_figure() {
    let dir = TempDir::new();
    let (repo, snapshot) = hub_repo(dir.path(), "pipeline");
    write(
        &snapshot.join("model_index.json"),
        br#"{"_class_name":"StableDiffusionPipeline"}"#,
    );
    linked(
        &repo,
        &snapshot,
        "unet/diffusion_pytorch_model.safetensors",
        "unet",
        100,
    );

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "pipeline");
    assert_eq!(model.footprint_bytes, 100);
    assert_eq!(model.serving_bytes, None);
}

#[test]
fn a_precision_variant_index_serves_its_whole_shard_set() {
    let index = br#"{"weight_map":{"a":"model.fp32-00001-of-00002.safetensors","b":"model.fp32-00002-of-00002.safetensors"}}"#;
    let config = br#"{"architectures":["WhisperForConditionalGeneration"]}"#;

    let dir = TempDir::new();
    let (repo, snapshot) = hub_repo(dir.path(), "variant");
    linked(
        &repo,
        &snapshot,
        "model.fp32-00001-of-00002.safetensors",
        "s1",
        80,
    );
    linked(
        &repo,
        &snapshot,
        "model.fp32-00002-of-00002.safetensors",
        "s2",
        30,
    );
    // A half-precision copy beside it, which this set does not take in.
    linked(&repo, &snapshot, "model.safetensors", "half", 55);
    write(&snapshot.join("model.safetensors.index.fp32.json"), index);
    write(&snapshot.join("config.json"), config);

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "variant");
    // The index is a map of the set, not something serving loads, as the
    // default index is not.
    assert_eq!(model.serving_bytes, Some(110 + config.len() as i64));
    assert!(!model.downloading);
}

#[test]
fn an_index_of_a_variant_nobody_fetched_leaves_the_repo_ready() {
    let index = br#"{"weight_map":{"a":"model.fp16-00001-of-00002.safetensors","b":"model.fp16-00002-of-00002.safetensors"}}"#;
    let config = br#"{"architectures":["LlamaForCausalLM"],"model_type":"llama","quantization":{"group_size":64,"bits":4}}"#;

    let dir = TempDir::new();
    let (repo, snapshot) = hub_repo(dir.path(), "tiny");
    linked(&repo, &snapshot, "model.safetensors", "weight", 300);
    write(&snapshot.join("config.json"), config);
    write(&snapshot.join("tokenizer_config.json"), b"{}");
    // A `*.json` pattern fetched the half-precision index; none of its shards
    // were asked for.
    write(&snapshot.join("model.safetensors.index.fp16.json"), index);

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "tiny");
    assert!(
        !model.downloading,
        "a set with no member here was not chosen"
    );
    assert_eq!(
        model.serving_bytes,
        Some(300 + config.len() as i64 + 2),
        "the weight, its config and tokenizer config"
    );
}

#[cfg(unix)]
fn scan_within(scanner: HFCacheScanner) -> Option<ScanResult> {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(scanner.scan());
    });
    finished
        .recv_timeout(std::time::Duration::from_secs(10))
        .ok()
}

#[cfg(unix)]
fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo {}", path.display());
}

#[cfg(unix)]
#[test]
fn a_pipe_in_a_snapshot_never_blocks_the_scan() {
    let dir = TempDir::new();
    let (repo, snapshot) = hub_repo(dir.path(), "configpipe");
    linked(&repo, &snapshot, "model.safetensors", "weight", 30);
    mkfifo(&snapshot.join("config.json"));
    let (repo, snapshot) = hub_repo(dir.path(), "indexpipe");
    linked(
        &repo,
        &snapshot,
        "model-00001-of-00002.safetensors",
        "shard",
        30,
    );
    mkfifo(&snapshot.join("model.safetensors.index.json"));
    let (repo, snapshot) = hub_repo(dir.path(), "refpipe");
    linked(&repo, &snapshot, "a.Q4_K_M.gguf", "gguf", 30);
    std::fs::remove_file(repo.join("refs").join("main")).unwrap();
    mkfifo(&repo.join("refs").join("main"));

    let result = scan_within(HFCacheScanner::single(dir.path()))
        .expect("the scan finished rather than waiting on a pipe");
    for name in ["configpipe", "indexpipe", "refpipe"] {
        find(&result, name);
    }
}

#[test]
fn a_variant_set_missing_a_shard_is_downloading_and_has_no_serving_figure() {
    let index = br#"{"weight_map":{"a":"model.fp32-00001-of-00002.safetensors","b":"model.fp32-00002-of-00002.safetensors"}}"#;

    let dir = TempDir::new();
    let (repo, snapshot) = hub_repo(dir.path(), "partial");
    linked(
        &repo,
        &snapshot,
        "model.fp32-00001-of-00002.safetensors",
        "s1",
        80,
    );
    write(&snapshot.join("model.safetensors.index.fp32.json"), index);

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "partial");
    assert!(model.downloading);
    assert_eq!(model.serving_bytes, None, "the disk figure stands in");
}

#[test]
fn an_importance_matrix_beside_the_weight_is_not_counted_to_serve() {
    let dir = TempDir::new();
    let (repo, snapshot) = hub_repo(dir.path(), "quantized");
    linked(&repo, &snapshot, "a.Q4_K_M.gguf", "weight", 100);
    linked(&repo, &snapshot, "a.imatrix", "imatrix", 9);
    linked(&repo, &snapshot, "imatrix.dat", "dat", 8);
    linked(&repo, &snapshot, "config.json", "config", 7);

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "quantized");
    assert_eq!(model.footprint_bytes, 124);
    assert_eq!(model.serving_bytes, Some(107));
}

/// The files `model`'s footprint counts as its scanner listed them, and as
/// the shelf lists them again from the record it becomes, each sorted.
fn listed_both_ways(model: &DiscoveredModel) -> (Vec<String>, Vec<String>) {
    let record = kernel::records::ModelRecord::new(
        &model.name,
        kernel::records::Modality::text(),
        Vec::new(),
        model.source.clone(),
    );
    let mut scanned = model.files.clone();
    scanned.sort();
    let mut again: Vec<String> = kernel::discovery::footprint_files(&record)
        .unwrap_or_default()
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    again.sort();
    (scanned, again)
}

#[test]
fn lists_every_file_of_the_repo_the_way_the_shelf_lists_them() {
    let dir = TempDir::new();
    let repo = standard_repo(dir.path());
    blob(&repo, "config-blob", 12);
    std::fs::write(repo.join(".DS_Store"), b"\x00\x00\x00\x01Bud1").unwrap();

    let result = HFCacheScanner::single(dir.path()).scan();
    let model = find(&result, "Llama-3");
    let (scanned, again) = listed_both_ways(model);
    let at = |relative: &str| repo.join(relative).to_string_lossy().into_owned();
    assert_eq!(
        scanned,
        vec![
            at(".DS_Store"),
            at("blobs/config-blob"),
            at("blobs/weight-blob"),
            at("refs/main"),
            at("snapshots/abc123/config.json"),
            at("snapshots/abc123/model.safetensors"),
            at("snapshots/abc123/tokenizer.json"),
        ]
    );
    assert_eq!(again, scanned);
}
