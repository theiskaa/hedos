//! Tests for the on-disk store scanners, driven by fake store directories built
//! under a temp dir.

mod support;

use std::path::Path;

use kernel::discovery::{DiscoveredModel, OllamaStoreScanner, StoreScanner};
use kernel::records::{Capability, SourceKind};
use support::TempDir;

/// Write `contents` to `root/manifests/<registry>/<namespace>/<model>/<tag>`.
fn write_manifest(root: &Path, coords: [&str; 4], contents: &str) {
    let path = root
        .join("manifests")
        .join(coords[0])
        .join(coords[1])
        .join(coords[2])
        .join(coords[3]);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// Write a blob addressed by `digest` (e.g. `sha256:abc` → `blobs/sha256-abc`).
fn write_blob(root: &Path, digest: &str, contents: &[u8]) {
    let blobs = root.join("blobs");
    std::fs::create_dir_all(&blobs).unwrap();
    std::fs::write(blobs.join(digest.replace(':', "-")), contents).unwrap();
}

fn layer(media: &str, size: i64, digest: &str) -> String {
    format!("{{\"mediaType\":\"{media}\",\"size\":{size},\"digest\":\"{digest}\"}}")
}

fn manifest(layers: &[String]) -> String {
    format!("{{\"layers\":[{}]}}", layers.join(","))
}

fn only(models: &[DiscoveredModel]) -> &DiscoveredModel {
    assert_eq!(models.len(), 1, "expected one model: {models:?}");
    &models[0]
}

#[test]
fn scans_a_library_model_with_all_hints() {
    let dir = TempDir::new();
    let root = dir.path();
    write_manifest(
        root,
        ["registry.ollama.ai", "library", "llama3.2", "latest"],
        &manifest(&[
            layer("application/vnd.ollama.image.model", 100, "sha256:aaa"),
            layer("application/vnd.ollama.image.template", 5, "sha256:bbb"),
            layer("application/vnd.ollama.image.params", 10, "sha256:ccc"),
        ]),
    );
    write_blob(root, "sha256:aaa", b"not-a-real-gguf");
    write_blob(root, "sha256:ccc", br#"{"num_ctx":4096,"stop":["<end>"]}"#);

    let result = OllamaStoreScanner::new(root).scan();
    assert!(result.issues.is_empty(), "no issues: {:?}", result.issues);
    assert!(result.failed_kinds.is_empty());
    let model = only(&result.discovered);

    assert_eq!(model.name, "llama3.2:latest");
    assert_eq!(model.source.kind, SourceKind::ollama());
    assert_eq!(model.source.repo.as_deref(), Some("llama3.2:latest"));
    assert_eq!(model.footprint_bytes, 115);
    // A non-GGUF weight blob falls back to the plain chat profile.
    assert!(model.capabilities_hint.contains(&Capability::chat()));
    assert!(!model.capabilities_hint.contains(&Capability::see()));
    assert!(
        model
            .primary_weight_path
            .as_deref()
            .unwrap()
            .ends_with("sha256-aaa")
    );
    assert_eq!(model.context_length_hint, Some(4096));
    assert_eq!(model.has_chat_template_hint, Some(true));
    assert_eq!(
        model.stop_tokens_hint.as_deref(),
        Some(&["<end>".to_owned()][..])
    );
}

#[test]
fn reads_tool_support_from_the_go_template() {
    // A Go template that gates on `.Tools` is Ollama's own signal for tool
    // support; reading it is authoritative and needs no daemon.
    let dir = TempDir::new();
    let root = dir.path();
    write_manifest(
        root,
        ["registry.ollama.ai", "library", "toolful", "latest"],
        &manifest(&[
            layer("application/vnd.ollama.image.model", 1, "sha256:m1"),
            layer("application/vnd.ollama.image.template", 40, "sha256:t1"),
        ]),
    );
    write_blob(root, "sha256:m1", b"x");
    write_blob(
        root,
        "sha256:t1",
        b"{{ if .Tools }}{{ .Tools }}{{ end }}Assistant:",
    );

    let result = OllamaStoreScanner::new(root).scan();
    let model = only(&result.discovered);
    assert_eq!(model.tool_capable_hint, Some(true));
}

#[test]
fn a_template_without_tool_markers_is_reported_tool_incapable() {
    let dir = TempDir::new();
    let root = dir.path();
    write_manifest(
        root,
        ["registry.ollama.ai", "library", "plain", "latest"],
        &manifest(&[
            layer("application/vnd.ollama.image.model", 1, "sha256:m2"),
            layer("application/vnd.ollama.image.template", 20, "sha256:t2"),
        ]),
    );
    write_blob(root, "sha256:m2", b"x");
    write_blob(root, "sha256:t2", b"User: {{ .Prompt }}\nAssistant:");

    let result = OllamaStoreScanner::new(root).scan();
    let model = only(&result.discovered);
    // The authoritative negative — the case that steers the launch picker away.
    assert_eq!(model.tool_capable_hint, Some(false));
}

#[test]
fn a_model_without_a_template_layer_leaves_tool_support_undetermined() {
    let dir = TempDir::new();
    let root = dir.path();
    write_manifest(
        root,
        ["registry.ollama.ai", "library", "bare", "latest"],
        &manifest(&[layer("application/vnd.ollama.image.model", 1, "sha256:m3")]),
    );
    write_blob(root, "sha256:m3", b"x");

    let result = OllamaStoreScanner::new(root).scan();
    let model = only(&result.discovered);
    // Undetermined, not "no" — so a model whose template rides inside the GGUF
    // (e.g. qwen) is assumed capable rather than hidden.
    assert_eq!(model.tool_capable_hint, None);
}

#[test]
fn keeps_a_non_library_namespace_in_the_name() {
    let dir = TempDir::new();
    write_manifest(
        dir.path(),
        ["registry.ollama.ai", "acme", "wizard", "v2"],
        &manifest(&[layer("application/vnd.ollama.image.model", 1, "sha256:a")]),
    );
    write_blob(dir.path(), "sha256:a", b"x");

    let result = OllamaStoreScanner::new(dir.path()).scan();
    assert_eq!(only(&result.discovered).name, "acme/wizard:v2");
}

#[test]
fn a_projector_layer_marks_the_model_vision_capable() {
    let dir = TempDir::new();
    write_manifest(
        dir.path(),
        ["registry.ollama.ai", "library", "llava", "latest"],
        &manifest(&[
            layer("application/vnd.ollama.image.model", 1, "sha256:a"),
            layer("application/vnd.ollama.image.projector", 1, "sha256:b"),
        ]),
    );
    write_blob(dir.path(), "sha256:a", b"x");

    let result = OllamaStoreScanner::new(dir.path()).scan();
    assert!(
        only(&result.discovered)
            .capabilities_hint
            .contains(&Capability::see())
    );
}

#[test]
fn a_malformed_manifest_becomes_an_issue_not_a_model() {
    let dir = TempDir::new();
    write_manifest(
        dir.path(),
        ["registry.ollama.ai", "library", "broken", "latest"],
        "{ not json",
    );

    let result = OllamaStoreScanner::new(dir.path()).scan();
    assert!(result.discovered.is_empty());
    assert_eq!(result.issues.len(), 1);
    assert!(result.issues[0].contains("unreadable manifest"));
}

#[test]
fn an_unreadable_params_blob_becomes_an_issue() {
    let dir = TempDir::new();
    // The params layer references a blob that was never written.
    write_manifest(
        dir.path(),
        ["registry.ollama.ai", "library", "noparams", "latest"],
        &manifest(&[
            layer("application/vnd.ollama.image.model", 1, "sha256:a"),
            layer("application/vnd.ollama.image.params", 1, "sha256:missing"),
        ]),
    );
    write_blob(dir.path(), "sha256:a", b"x");

    let result = OllamaStoreScanner::new(dir.path()).scan();
    assert_eq!(result.discovered.len(), 1, "model still discovered");
    assert!(
        result
            .issues
            .iter()
            .any(|issue| issue.contains("params blob"))
    );
}

#[test]
fn skips_paths_that_are_not_four_components() {
    let dir = TempDir::new();
    // A manifest file only two levels under `manifests/` is not a valid coord.
    let stray = dir.path().join("manifests").join("registry.ollama.ai");
    std::fs::create_dir_all(&stray).unwrap();
    std::fs::write(stray.join("loose"), manifest(&[])).unwrap();

    let result = OllamaStoreScanner::new(dir.path()).scan();
    assert!(result.discovered.is_empty());
    assert!(result.issues.is_empty());
}

#[test]
fn a_missing_root_scans_to_nothing() {
    let dir = TempDir::new();
    let result = OllamaStoreScanner::new(dir.path().join("nope")).scan();
    assert!(result.discovered.is_empty());
    assert!(result.issues.is_empty());
    assert!(result.failed_kinds.is_empty());
}

#[test]
fn a_root_without_a_manifests_dir_scans_to_nothing() {
    let dir = TempDir::new();
    std::fs::create_dir_all(dir.path().join("blobs")).unwrap();
    let result = OllamaStoreScanner::new(dir.path()).scan();
    assert!(result.discovered.is_empty());
    assert!(result.failed_kinds.is_empty());
}

#[test]
fn a_root_that_is_not_a_directory_fails_the_kind() {
    // The root exists but can't be listed (it's a regular file) — a scan failure,
    // not an empty store.
    let dir = TempDir::new();
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, b"x").unwrap();
    let result = OllamaStoreScanner::new(file).scan();
    assert!(result.discovered.is_empty());
    assert_eq!(result.failed_kinds, vec![SourceKind::ollama()]);
}

#[test]
fn a_manifests_path_that_is_a_file_fails_the_kind() {
    let dir = TempDir::new();
    std::fs::create_dir_all(dir.path()).unwrap();
    // `manifests` exists but is a file, so it can't be walked.
    std::fs::write(dir.path().join("manifests"), b"x").unwrap();
    let result = OllamaStoreScanner::new(dir.path()).scan();
    assert_eq!(result.failed_kinds, vec![SourceKind::ollama()]);
}

#[test]
fn a_non_object_params_blob_is_an_issue_but_still_discovers() {
    let dir = TempDir::new();
    write_manifest(
        dir.path(),
        ["registry.ollama.ai", "library", "weird", "latest"],
        &manifest(&[
            layer("application/vnd.ollama.image.model", 1, "sha256:a"),
            layer("application/vnd.ollama.image.params", 1, "sha256:p"),
        ]),
    );
    write_blob(dir.path(), "sha256:a", b"x");
    write_blob(dir.path(), "sha256:p", b"[1,2,3]"); // valid JSON, not an object

    let result = OllamaStoreScanner::new(dir.path()).scan();
    assert_eq!(result.discovered.len(), 1);
    assert!(
        result
            .issues
            .iter()
            .any(|issue| issue.contains("params blob"))
    );
    assert_eq!(only(&result.discovered).context_length_hint, None);
}

#[test]
fn a_zero_num_ctx_is_dropped() {
    let dir = TempDir::new();
    write_manifest(
        dir.path(),
        ["registry.ollama.ai", "library", "zero", "latest"],
        &manifest(&[
            layer("application/vnd.ollama.image.model", 1, "sha256:a"),
            layer("application/vnd.ollama.image.params", 1, "sha256:p"),
        ]),
    );
    write_blob(dir.path(), "sha256:a", b"x");
    write_blob(dir.path(), "sha256:p", br#"{"num_ctx":0}"#);

    let result = OllamaStoreScanner::new(dir.path()).scan();
    assert!(result.issues.is_empty());
    assert_eq!(only(&result.discovered).context_length_hint, None);
}

#[test]
fn a_model_without_a_weight_layer_has_no_weight_path() {
    let dir = TempDir::new();
    write_manifest(
        dir.path(),
        ["registry.ollama.ai", "library", "bare", "latest"],
        &manifest(&[layer(
            "application/vnd.ollama.image.template",
            1,
            "sha256:t",
        )]),
    );

    let result = OllamaStoreScanner::new(dir.path()).scan();
    let model = only(&result.discovered);
    assert_eq!(model.primary_weight_path, None);
    // No weight → the plain chat profile.
    assert!(model.capabilities_hint.contains(&Capability::chat()));
    assert!(!model.capabilities_hint.contains(&Capability::see()));
}

#[test]
fn discovers_multiple_models_in_one_store() {
    let dir = TempDir::new();
    for name in ["alpha", "beta", "gamma"] {
        write_manifest(
            dir.path(),
            ["registry.ollama.ai", "library", name, "latest"],
            &manifest(&[layer("application/vnd.ollama.image.model", 1, "sha256:a")]),
        );
    }
    write_blob(dir.path(), "sha256:a", b"x");

    let mut names: Vec<String> = OllamaStoreScanner::new(dir.path())
        .scan()
        .discovered
        .into_iter()
        .map(|model| model.name)
        .collect();
    names.sort();
    assert_eq!(names, ["alpha:latest", "beta:latest", "gamma:latest"]);
}

#[test]
fn skips_paths_deeper_than_four_components() {
    let dir = TempDir::new();
    // A five-deep path is not a valid `<registry>/<ns>/<model>/<tag>`.
    let deep = dir
        .path()
        .join("manifests")
        .join("registry.ollama.ai")
        .join("library")
        .join("model")
        .join("tag")
        .join("extra");
    std::fs::create_dir_all(deep.parent().unwrap()).unwrap();
    std::fs::write(deep, manifest(&[])).unwrap();

    let result = OllamaStoreScanner::new(dir.path()).scan();
    assert!(result.discovered.is_empty());
    assert!(result.issues.is_empty());
}
