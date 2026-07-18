//! Tests for manifest support: path resolution, placeholder expansion, command
//! substitution, and the string/JSON helpers.

mod support;

use std::collections::BTreeMap;
use std::path::Path;

use kernel::records::{JsonValue, Modality, ModelRecord, ModelSource, SourceKind};
use runtime::manifests::{
    MAX_OUTPUT_FILE_BYTES, SidecarModelPaths, bounded_output_data, conversation_text,
    error_summary, expand_placeholders, prompt_text, slug, substituted,
};
use support::TempDir;

fn record(kind: SourceKind, path: &str) -> ModelRecord {
    ModelRecord::new(
        "m",
        Modality::text(),
        Vec::new(),
        ModelSource::new(kind, path),
    )
}

fn obj(pairs: Vec<(&str, JsonValue)>) -> JsonValue {
    JsonValue::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn s(value: &str) -> JsonValue {
    JsonValue::String(value.to_owned())
}

fn placeholders(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[test]
fn sidecar_paths_for_a_plain_file_are_the_path_itself() {
    let dir = TempDir::new();
    let model = dir.path().join("model.gguf");
    std::fs::write(&model, b"GGUF").unwrap();
    let paths = SidecarModelPaths::resolve(&record(SourceKind::file(), model.to_str().unwrap()));
    assert_eq!(paths.sandbox_root, paths.snapshot);
}

#[test]
fn sidecar_paths_resolve_a_hugging_face_snapshot() {
    let dir = TempDir::new();
    let snapshot = dir.path().join("snapshots").join("rev1");
    std::fs::create_dir_all(&snapshot).unwrap();
    let mut rec = record(
        SourceKind::huggingface_cache(),
        dir.path().to_str().unwrap(),
    );
    rec.source.reference = Some("rev1".to_owned());

    let paths = SidecarModelPaths::resolve(&rec);
    assert!(paths.snapshot.ends_with("snapshots/rev1"));
    assert_ne!(paths.sandbox_root, paths.snapshot);
    // The sandbox root is still the base container.
    assert!(paths.snapshot.starts_with(&paths.sandbox_root));
}

#[test]
fn sidecar_paths_fall_back_when_the_snapshot_is_absent() {
    let dir = TempDir::new();
    let mut rec = record(
        SourceKind::huggingface_cache(),
        dir.path().to_str().unwrap(),
    );
    rec.source.reference = Some("missing".to_owned());
    let paths = SidecarModelPaths::resolve(&rec);
    assert_eq!(paths.sandbox_root, paths.snapshot);
}

#[test]
fn expand_placeholders_replaces_known_keys_and_passes_the_rest() {
    let repl = placeholders(&[("{model}", "/w/model"), ("{prompt}", "hi")]);
    assert_eq!(
        expand_placeholders("--model={model}", &repl),
        "--model=/w/model"
    );
    // An unknown placeholder is left verbatim.
    assert_eq!(expand_placeholders("{unknown}", &repl), "{unknown}");
}

#[test]
fn expand_placeholders_prefers_the_longest_matching_key() {
    // "ab" must win over its prefix "a" at the same position.
    let repl = placeholders(&[("a", "X"), ("ab", "Y")]);
    assert_eq!(expand_placeholders("ab", &repl), "Y");
    assert_eq!(expand_placeholders("ac", &repl), "Xc");
}

#[test]
fn expand_placeholders_is_single_pass_and_utf8_safe() {
    // A replacement value that itself looks like a placeholder is NOT re-expanded.
    let repl = placeholders(&[("{a}", "{b}"), ("{b}", "SHOULD-NOT-APPEAR")]);
    assert_eq!(expand_placeholders("{a}", &repl), "{b}");
    // Multibyte text around a placeholder boundary stays intact.
    let repl = placeholders(&[("{m}", "→café")]);
    assert_eq!(expand_placeholders("π{m}ω", &repl), "π→caféω");
}

#[test]
fn prompt_text_prefers_the_prompt_then_falls_back_to_the_conversation() {
    assert_eq!(prompt_text(&obj(vec![("prompt", s("direct"))])), "direct");
    let convo = obj(vec![(
        "messages",
        JsonValue::Array(vec![
            obj(vec![("role", s("user")), ("content", s("hello"))]),
            obj(vec![("role", s("assistant")), ("content", s("hi"))]),
        ]),
    )]);
    assert_eq!(prompt_text(&convo), "user: hello\nassistant: hi");
}

#[test]
fn conversation_text_skips_malformed_entries() {
    let convo = obj(vec![(
        "messages",
        JsonValue::Array(vec![
            obj(vec![("role", s("user")), ("content", s("ok"))]),
            obj(vec![("role", s("user"))]), // no content → skipped
            JsonValue::String("not an object".to_owned()),
        ]),
    )]);
    assert_eq!(conversation_text(&convo), "user: ok");
    // A payload with no messages is empty.
    assert_eq!(conversation_text(&obj(vec![])), "");
}

#[test]
fn error_summary_takes_the_last_nonblank_line_capped() {
    assert_eq!(error_summary("first\n\n  last error  \n\n"), "last error");
    assert_eq!(
        error_summary("   \n  "),
        "the runtime stopped without output"
    );
    let long = "x".repeat(400);
    assert_eq!(error_summary(&long).chars().count(), 300);
}

#[test]
fn error_summary_splits_carriage_return_progress_bars() {
    // tqdm-style output: `\r`-separated segments, the last is the real error.
    let raw = "loading\rstep 1/5\rFATAL: CUDA out of memory";
    assert_eq!(error_summary(raw), "FATAL: CUDA out of memory");
    // A trailing CRLF blank line is still ignored.
    assert_eq!(error_summary("a\r\nboom\r\n"), "boom");
}

#[test]
fn slug_replaces_non_alphanumerics() {
    assert_eq!(slug("python:mlx-lm@1.0"), "python-mlx-lm-1-0");
    assert_eq!(slug("abc123"), "abc123");
}

#[test]
fn bounded_output_data_reads_under_the_cap_and_refuses_over_it() {
    let dir = TempDir::new();
    let file = dir.path().join("out.bin");
    std::fs::write(&file, b"payload").unwrap();
    assert_eq!(
        bounded_output_data(&file, MAX_OUTPUT_FILE_BYTES).unwrap(),
        b"payload"
    );
    // A limit below the file size is refused.
    let err = bounded_output_data(&file, 3).unwrap_err();
    assert!(err.to_string().contains("larger than"));

    // A missing file (size defaults to 0, under the cap) surfaces the read error.
    let missing = dir.path().join("nope.bin");
    let err = bounded_output_data(&missing, MAX_OUTPUT_FILE_BYTES).unwrap_err();
    assert!(err.to_string().contains("reading output"));
}

#[test]
fn a_ref_on_a_non_hugging_face_record_is_ignored() {
    let dir = TempDir::new();
    // A `snapshots/rev` exists, but the kind is `file`, not huggingface_cache —
    // so the ref is ignored and the snapshot equals the root.
    std::fs::create_dir_all(dir.path().join("snapshots").join("rev")).unwrap();
    let mut rec = record(SourceKind::file(), dir.path().to_str().unwrap());
    rec.source.reference = Some("rev".to_owned());
    let paths = SidecarModelPaths::resolve(&rec);
    assert_eq!(paths.sandbox_root, paths.snapshot);
}

#[test]
fn substituted_rejects_an_empty_command() {
    let rec = record(SourceKind::file(), "/m");
    let payload = obj(vec![]);
    let err = substituted(
        "   ",
        &rec,
        &payload,
        Path::new("/w"),
        Path::new("/o"),
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("empty"));
}

#[test]
fn substituted_expands_a_command_into_tokens() {
    let dir = TempDir::new();
    let model = dir.path().join("model.gguf");
    std::fs::write(&model, b"GGUF").unwrap();
    let rec = record(SourceKind::file(), model.to_str().unwrap());
    let payload = obj(vec![("prompt", s("say hi"))]);
    let workdir = Path::new("/work");
    let outputs = Path::new("/work/out");

    let tokens = substituted(
        "run --model {model} --out {outputs}",
        &rec,
        &payload,
        workdir,
        outputs,
        None,
    )
    .unwrap();
    assert_eq!(tokens[0], "run");
    assert_eq!(tokens[1], "--model");
    assert!(tokens[2].ends_with("model.gguf"));
    assert_eq!(tokens[3], "--out");
    assert_eq!(tokens[4], "/work/out");
}

#[test]
fn substituted_collapses_runs_of_spaces() {
    let rec = record(SourceKind::file(), "/m");
    let payload = obj(vec![]);
    let tokens = substituted(
        "a   b",
        &rec,
        &payload,
        Path::new("/w"),
        Path::new("/o"),
        None,
    )
    .unwrap();
    assert_eq!(tokens, vec!["a".to_owned(), "b".to_owned()]);
}

#[test]
fn substituted_requires_an_env_for_the_python_placeholder() {
    let rec = record(SourceKind::file(), "/m");
    let payload = obj(vec![]);
    let err = substituted(
        "{python} run.py",
        &rec,
        &payload,
        Path::new("/w"),
        Path::new("/o"),
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("{python}"));

    // With an env dir the python placeholder resolves.
    let tokens = substituted(
        "{python} run.py",
        &rec,
        &payload,
        Path::new("/w"),
        Path::new("/o"),
        Some(Path::new("/env")),
    )
    .unwrap();
    assert_eq!(tokens[0], "/env/bin/python");
    assert_eq!(tokens[1], "run.py");
}
