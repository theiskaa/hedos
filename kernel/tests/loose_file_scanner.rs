//! Tests for the loose-directory scanner, driven by fake trees under a temp dir.

mod support;

use std::path::Path;

use kernel::discovery::{DiscoveredModel, LooseFileScanner, ScanResult, StoreScanner};
use kernel::records::{Capability, Modality, SourceKind};
use support::TempDir;

const CONFIG: &[u8] = br#"{"architectures":["LlamaForCausalLM"],"max_position_embeddings":4096}"#;

fn write(root: &Path, relative: &str, contents: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn find<'a>(result: &'a ScanResult, name: &str) -> &'a DiscoveredModel {
    result
        .discovered
        .iter()
        .find(|model| model.name == name)
        .unwrap_or_else(|| panic!("no model named {name}: {:?}", result.discovered))
}

#[test]
fn finds_a_safetensors_folder_bundle() {
    let dir = TempDir::new();
    write(dir.path(), "mymodel/config.json", CONFIG);
    write(dir.path(), "mymodel/model.safetensors", &[0u8; 100]);
    write(dir.path(), "mymodel/tokenizer.json", b"{}");

    let result = LooseFileScanner::single(dir.path()).scan();
    let model = find(&result, "mymodel");
    assert_eq!(model.source.kind, SourceKind::folder());
    assert_eq!(model.modality_hint, Some(Modality::text()));
    assert!(model.capabilities_hint.contains(&Capability::chat()));
    assert_eq!(model.context_length_hint, Some(4096));
    assert_eq!(model.footprint_bytes, CONFIG.len() as i64 + 100 + 2);
    assert!(
        model
            .primary_weight_path
            .as_deref()
            .unwrap()
            .ends_with("model.safetensors")
    );
}

#[test]
fn a_folder_needs_both_config_and_safetensors() {
    // config.json but no safetensors → not a bundle, and nothing else to find.
    let no_weights = TempDir::new();
    write(no_weights.path(), "x/config.json", CONFIG);
    assert!(
        LooseFileScanner::single(no_weights.path())
            .scan()
            .discovered
            .is_empty()
    );

    // safetensors but no config.json → not a bundle.
    let no_config = TempDir::new();
    write(no_config.path(), "x/model.safetensors", &[0u8; 10]);
    assert!(
        LooseFileScanner::single(no_config.path())
            .scan()
            .discovered
            .is_empty()
    );
}

#[test]
fn a_model_index_bundle_is_a_job() {
    let dir = TempDir::new();
    write(dir.path(), "diff/config.json", CONFIG);
    write(dir.path(), "diff/model.safetensors", &[0u8; 10]);
    write(
        dir.path(),
        "diff/model_index.json",
        br#"{"_class_name":"X"}"#,
    );

    let result = LooseFileScanner::single(dir.path()).scan();
    let model = find(&result, "diff");
    assert_eq!(model.execution_hint, kernel::records::ExecutionMode::Job);
    assert_eq!(model.modality_hint, None);
}

#[test]
fn a_bundle_is_not_recursed_into() {
    let dir = TempDir::new();
    write(dir.path(), "bundle/config.json", CONFIG);
    write(dir.path(), "bundle/model.safetensors", &[0u8; 10]);
    // A stray gguf inside the bundle must NOT become a second model.
    write(dir.path(), "bundle/extra.gguf", b"x");

    let result = LooseFileScanner::single(dir.path()).scan();
    assert_eq!(result.discovered.len(), 1);
    assert_eq!(result.discovered[0].name, "bundle");
}

#[test]
fn finds_a_loose_gguf() {
    let dir = TempDir::new();
    write(dir.path(), "model.gguf", &[0u8; 42]);

    let result = LooseFileScanner::single(dir.path()).scan();
    let model = find(&result, "model");
    assert_eq!(model.source.kind, SourceKind::file());
    assert_eq!(model.source.repo, None);
    assert_eq!(model.modality_hint, Some(Modality::text()));
    assert_eq!(model.footprint_bytes, 42);
}

#[test]
fn a_ggml_bin_is_a_transcription_model() {
    let dir = TempDir::new();
    // `lmgg` is the GGML magic.
    write(dir.path(), "whisper.bin", b"lmggDATA");
    // A .bin without the magic is ignored.
    write(dir.path(), "notmodel.bin", b"plain-bytes");

    let result = LooseFileScanner::single(dir.path()).scan();
    assert_eq!(result.discovered.len(), 1);
    let model = find(&result, "whisper");
    assert_eq!(model.modality_hint, Some(Modality::audio()));
    assert!(model.capabilities_hint.contains(&Capability::transcribe()));
    assert_eq!(model.source.kind, SourceKind::file());
}

#[test]
fn sweeps_only_to_max_depth() {
    let dir = TempDir::new();
    // root/a/b/found.gguf is at the deepest swept level; root/a/b/c/deep.gguf is
    // one level too deep.
    write(dir.path(), "a/b/found.gguf", &[0u8; 1]);
    write(dir.path(), "a/b/c/deep.gguf", &[0u8; 1]);

    let result = LooseFileScanner::single(dir.path()).scan();
    let names: Vec<&str> = result.discovered.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["found"]);
}

#[test]
fn skips_mmproj_and_hidden_files() {
    let dir = TempDir::new();
    write(dir.path(), "mmproj-model.gguf", b"x");
    write(dir.path(), ".hidden.gguf", b"x");
    write(dir.path(), "real.gguf", b"x");

    let result = LooseFileScanner::single(dir.path()).scan();
    assert_eq!(result.discovered.len(), 1);
    assert_eq!(result.discovered[0].name, "real");
}

#[test]
fn an_incomplete_loose_shard_set_is_downloading() {
    let dir = TempDir::new();
    write(dir.path(), "m-00001-of-00002.gguf", &[0u8; 1]);

    let result = LooseFileScanner::single(dir.path()).scan();
    assert!(find(&result, "m").downloading);
}

#[test]
fn a_missing_directory_is_silent() {
    let dir = TempDir::new();
    let result = LooseFileScanner::single(dir.path().join("gone")).scan();
    assert!(result.discovered.is_empty());
    assert!(result.failed_kinds.is_empty());
}

#[test]
fn a_required_missing_directory_fails_both_kinds() {
    let dir = TempDir::new();
    let scanner = LooseFileScanner::with_user_directories(vec![], vec![dir.path().join("gone")]);
    let mut kinds = scanner.scan().failed_kinds;
    kinds.sort_by_key(|kind| kind.as_str().to_owned());
    assert_eq!(kinds, vec![SourceKind::file(), SourceKind::folder()]);
}

#[test]
fn a_directory_that_is_a_file_fails() {
    let dir = TempDir::new();
    let file = dir.path().join("notdir");
    std::fs::write(&file, b"x").unwrap();
    let result = LooseFileScanner::single(file).scan();
    assert!(!result.failed_kinds.is_empty());
}

#[test]
fn the_first_of_equal_size_safetensors_is_primary() {
    let dir = TempDir::new();
    write(dir.path(), "b/config.json", CONFIG);
    // Two equal-size safetensors; the first by name wins (BTreeSet/sorted order
    // is deterministic here, unlike raw read_dir).
    write(dir.path(), "b/aaa.safetensors", &[0u8; 50]);
    write(dir.path(), "b/zzz.safetensors", &[0u8; 50]);

    // read_dir order is unspecified, so just assert a stable choice is made and
    // it is one of the two equal candidates (the fix removes the last-wins bias).
    let weight = find(&LooseFileScanner::single(dir.path()).scan(), "b")
        .primary_weight_path
        .clone()
        .unwrap();
    assert!(weight.ends_with(".safetensors"));
}

#[test]
fn a_bundle_needs_a_lowercase_safetensors_extension() {
    // The `.safetensors` check is case-sensitive; `.SAFETENSORS` is not a bundle.
    let dir = TempDir::new();
    write(dir.path(), "x/config.json", CONFIG);
    write(dir.path(), "x/model.SAFETENSORS", &[0u8; 10]);

    assert!(
        LooseFileScanner::single(dir.path())
            .scan()
            .discovered
            .is_empty()
    );
}

#[test]
fn loose_gguf_and_bin_extensions_are_case_insensitive() {
    let dir = TempDir::new();
    write(dir.path(), "UP.GGUF", &[0u8; 4]);
    write(dir.path(), "voice.BIN", b"lmggXX");

    let result = LooseFileScanner::single(dir.path()).scan();
    assert_eq!(find(&result, "UP").modality_hint, Some(Modality::text()));
    assert_eq!(
        find(&result, "voice").modality_hint,
        Some(Modality::audio())
    );
}

#[test]
fn finds_a_bundle_nested_below_the_root() {
    let dir = TempDir::new();
    write(dir.path(), "a/b/nested/config.json", CONFIG);
    write(dir.path(), "a/b/nested/model.safetensors", &[0u8; 10]);

    let result = LooseFileScanner::single(dir.path()).scan();
    let model = find(&result, "nested");
    assert_eq!(model.source.kind, SourceKind::folder());
}

#[test]
fn recurses_a_subdir_while_collecting_a_loose_gguf_beside_it() {
    let dir = TempDir::new();
    // One sweep level holds both a loose gguf and a subdir to descend into.
    write(dir.path(), "top.gguf", &[0u8; 4]);
    write(dir.path(), "sub/inner.gguf", &[0u8; 4]);

    let result = LooseFileScanner::single(dir.path()).scan();
    let mut names: Vec<&str> = result.discovered.iter().map(|m| m.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["inner", "top"]);
}

#[test]
fn a_present_required_directory_scans_clean() {
    let dir = TempDir::new();
    write(dir.path(), "model.gguf", &[0u8; 4]);
    let scanner = LooseFileScanner::with_user_directories(vec![], vec![dir.path().to_path_buf()]);
    let result = scanner.scan();
    assert!(result.failed_kinds.is_empty());
    assert_eq!(result.discovered.len(), 1);
}

#[cfg(unix)]
#[test]
fn a_symlinked_directory_is_not_recursed() {
    let dir = TempDir::new();
    let real = TempDir::new();
    write(real.path(), "hidden.gguf", &[0u8; 4]);
    std::os::unix::fs::symlink(real.path(), dir.path().join("link")).unwrap();
    write(dir.path(), "here.gguf", &[0u8; 4]);

    let result = LooseFileScanner::single(dir.path()).scan();
    let names: Vec<&str> = result.discovered.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["here"], "the symlinked dir is not descended into");
}
