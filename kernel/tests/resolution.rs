//! Integration tests for the `resolution` format parsers: the GGUF header reader
//! (architecture, context length, chat template, value skipping), the
//! architecture profile table, and safetensors MLX detection. Byte buffers are
//! constructed in-test so no real model files are needed.

mod support;

use std::fs;

use kernel::records::{Capability, ExecutionMode, Modality};
use kernel::resolution::{
    ModelFormat, gguf_architecture_profile, gguf_facts, gguf_general_architecture, has_ggml_magic,
    has_gguf_magic, ollama_chat_profile, ollama_vision_profile, safetensors_format,
    safetensors_header_format,
};
use support::TempDir;

fn gguf_string(value: &str) -> Vec<u8> {
    let mut bytes = (value.len() as u64).to_le_bytes().to_vec();
    bytes.extend_from_slice(value.as_bytes());
    bytes
}

fn kv_string(key: &str, value: &str) -> Vec<u8> {
    let mut bytes = gguf_string(key);
    bytes.extend_from_slice(&8u32.to_le_bytes());
    bytes.extend(gguf_string(value));
    bytes
}

fn kv_u32(key: &str, value: u32) -> Vec<u8> {
    let mut bytes = gguf_string(key);
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&value.to_le_bytes());
    bytes
}

fn kv_bool(key: &str, value: bool) -> Vec<u8> {
    let mut bytes = gguf_string(key);
    bytes.extend_from_slice(&7u32.to_le_bytes());
    bytes.push(value as u8);
    bytes
}

fn kv_u32_array(key: &str, values: &[u32]) -> Vec<u8> {
    let mut bytes = gguf_string(key);
    bytes.extend_from_slice(&9u32.to_le_bytes());
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&(values.len() as u64).to_le_bytes());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn build_gguf(version: u32, kvs: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = b"GGUF".to_vec();
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(&(kvs.len() as u64).to_le_bytes());
    for kv in kvs {
        bytes.extend_from_slice(kv);
    }
    bytes
}

fn write(dir: &TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn gguf_facts_reads_architecture_context_and_template() {
    let dir = TempDir::new();
    let gguf = build_gguf(
        3,
        &[
            kv_string("general.architecture", "llama"),
            kv_u32("llama.context_length", 4096),
            kv_string("tokenizer.chat_template", "{{ messages }}"),
        ],
    );
    let facts = gguf_facts(&write(&dir, "model.gguf", &gguf)).unwrap();
    assert_eq!(facts.architecture.as_deref(), Some("llama"));
    assert_eq!(facts.context_length, Some(4096));
    assert!(facts.has_chat_template);
}

#[test]
fn gguf_skips_unwanted_values_before_the_architecture() {
    let dir = TempDir::new();
    let gguf = build_gguf(
        3,
        &[
            kv_bool("general.some_flag", true),
            kv_u32_array("general.some_list", &[1, 2, 3, 4]),
            kv_string("general.architecture", "qwen2"),
        ],
    );
    let facts = gguf_facts(&write(&dir, "model.gguf", &gguf)).unwrap();
    assert_eq!(facts.architecture.as_deref(), Some("qwen2"));
    assert!(!facts.has_chat_template);
    assert_eq!(facts.context_length, None);
}

#[test]
fn gguf_uses_the_sole_context_length_when_no_arch_match() {
    let dir = TempDir::new();
    let gguf = build_gguf(
        3,
        &[
            kv_string("general.architecture", "llama"),
            kv_u32("other.context_length", 2048),
        ],
    );
    let facts = gguf_facts(&write(&dir, "model.gguf", &gguf)).unwrap();
    assert_eq!(facts.context_length, Some(2048));
}

#[test]
fn gguf_prefers_the_matching_architecture_context_length() {
    let dir = TempDir::new();
    let gguf = build_gguf(
        3,
        &[
            kv_string("general.architecture", "llama"),
            kv_u32("llama.context_length", 8192),
            kv_u32("clip.context_length", 512),
        ],
    );
    let facts = gguf_facts(&write(&dir, "model.gguf", &gguf)).unwrap();
    assert_eq!(facts.context_length, Some(8192));
}

#[test]
fn gguf_ignores_ambiguous_multiple_context_lengths() {
    let dir = TempDir::new();
    let gguf = build_gguf(
        3,
        &[
            kv_u32("a.context_length", 1024),
            kv_u32("b.context_length", 2048),
        ],
    );
    let facts = gguf_facts(&write(&dir, "model.gguf", &gguf)).unwrap();
    assert_eq!(facts.architecture, None);
    assert_eq!(facts.context_length, None);
}

#[test]
fn gguf_version_one_and_garbage_are_rejected() {
    let dir = TempDir::new();
    let v1 = build_gguf(1, &[kv_string("general.architecture", "llama")]);
    assert!(gguf_facts(&write(&dir, "v1.gguf", &v1)).is_none());
    assert!(gguf_facts(&write(&dir, "junk.gguf", b"not a gguf file")).is_none());
}

#[test]
fn magic_bytes_are_detected() {
    let dir = TempDir::new();
    let gguf = write(&dir, "m.gguf", &build_gguf(3, &[]));
    assert!(has_gguf_magic(&gguf));
    assert!(!has_ggml_magic(&gguf));

    let ggml = write(&dir, "m.bin", b"lmgg\x00\x00\x00\x00");
    assert!(has_ggml_magic(&ggml));
    assert!(!has_gguf_magic(&ggml));

    assert!(!has_gguf_magic(&dir.join("missing.gguf")));
}

#[test]
fn general_architecture_helper_reads_the_name() {
    let dir = TempDir::new();
    let gguf = build_gguf(3, &[kv_string("general.architecture", "gemma")]);
    assert_eq!(
        gguf_general_architecture(&write(&dir, "m.gguf", &gguf)).as_deref(),
        Some("gemma")
    );
}

#[test]
fn architecture_profiles_map_known_names() {
    let whisper = gguf_architecture_profile("whisper").unwrap();
    assert_eq!(whisper.modality, Modality::audio());
    assert_eq!(whisper.capabilities, vec![Capability::transcribe()]);
    assert_eq!(whisper.execution, ExecutionMode::Stream);

    let vision = gguf_architecture_profile("qwen2vl").unwrap();
    assert!(vision.capabilities.contains(&Capability::see()));

    let clip = gguf_architecture_profile("clip").unwrap();
    assert_eq!(clip.modality, Modality::vision());
    assert!(clip.capabilities.is_empty());
    assert_eq!(clip.execution, ExecutionMode::Sync);

    let bert = gguf_architecture_profile("bert").unwrap();
    assert_eq!(bert.capabilities, vec![Capability::embed()]);

    assert!(gguf_architecture_profile("unknown-arch").is_none());
}

#[test]
fn ollama_default_profiles() {
    assert_eq!(
        ollama_chat_profile().capabilities,
        vec![Capability::chat(), Capability::complete()]
    );
    assert!(
        ollama_vision_profile()
            .capabilities
            .contains(&Capability::see())
    );
}

fn safetensors_bytes(header_json: &str) -> Vec<u8> {
    let mut bytes = (header_json.len() as u64).to_le_bytes().to_vec();
    bytes.extend_from_slice(header_json.as_bytes());
    bytes
}

#[test]
fn safetensors_header_format_reads_metadata() {
    let dir = TempDir::new();
    let mlx = write(
        &dir,
        "a.safetensors",
        &safetensors_bytes(r#"{"__metadata__":{"format":"mlx"}}"#),
    );
    assert_eq!(safetensors_header_format(&mlx).as_deref(), Some("mlx"));

    let plain = write(
        &dir,
        "b.safetensors",
        &safetensors_bytes(r#"{"weight":{"dtype":"F32"}}"#),
    );
    assert_eq!(safetensors_header_format(&plain), None);
}

#[test]
fn safetensors_format_detects_mlx_by_quantization_or_header() {
    let dir = TempDir::new();
    write(
        &dir,
        "model.safetensors",
        &safetensors_bytes(r#"{"__metadata__":{"format":"mlx"}}"#),
    );

    let quantized = write(&dir, "config.json", br#"{"quantization":{"bits":4}}"#);
    assert_eq!(
        safetensors_format(dir.path(), &quantized),
        Some(ModelFormat::MlxSafetensors)
    );

    let plain_config = write(&dir, "plain.json", br#"{"model_type":"llama"}"#);
    assert_eq!(
        safetensors_format(dir.path(), &plain_config),
        Some(ModelFormat::MlxSafetensors)
    );
}

#[test]
fn safetensors_format_is_plain_without_mlx_markers() {
    let dir = TempDir::new();
    write(
        &dir,
        "model.safetensors",
        &safetensors_bytes(r#"{"weight":{"dtype":"F32"}}"#),
    );
    let config = write(&dir, "config.json", br#"{"model_type":"llama"}"#);
    assert_eq!(
        safetensors_format(dir.path(), &config),
        Some(ModelFormat::Safetensors)
    );
}

#[test]
fn safetensors_format_needs_a_weight_file() {
    let dir = TempDir::new();
    let config = write(&dir, "config.json", br#"{"model_type":"llama"}"#);
    assert_eq!(safetensors_format(dir.path(), &config), None);
}

fn kv_typed(key: &str, type_code: u32, value: &[u8]) -> Vec<u8> {
    let mut bytes = gguf_string(key);
    bytes.extend_from_slice(&type_code.to_le_bytes());
    bytes.extend_from_slice(value);
    bytes
}

fn kv_string_array(key: &str, values: &[&str]) -> Vec<u8> {
    let mut bytes = gguf_string(key);
    bytes.extend_from_slice(&9u32.to_le_bytes());
    bytes.extend_from_slice(&8u32.to_le_bytes());
    bytes.extend_from_slice(&(values.len() as u64).to_le_bytes());
    for value in values {
        bytes.extend(gguf_string(value));
    }
    bytes
}

#[test]
fn gguf_version_two_is_accepted() {
    let dir = TempDir::new();
    let gguf = build_gguf(2, &[kv_string("general.architecture", "llama")]);
    assert_eq!(
        gguf_facts(&write(&dir, "m.gguf", &gguf))
            .unwrap()
            .architecture
            .as_deref(),
        Some("llama")
    );
}

#[test]
fn gguf_context_length_reads_signed_and_clamped_integers() {
    let dir = TempDir::new();
    let signed = build_gguf(
        3,
        &[
            kv_string("general.architecture", "llama"),
            kv_typed("llama.context_length", 11, &16_384i64.to_le_bytes()),
        ],
    );
    assert_eq!(
        gguf_facts(&write(&dir, "a.gguf", &signed))
            .unwrap()
            .context_length,
        Some(16_384)
    );

    let huge = build_gguf(
        3,
        &[
            kv_string("general.architecture", "llama"),
            kv_typed("llama.context_length", 10, &u64::MAX.to_le_bytes()),
        ],
    );
    assert_eq!(
        gguf_facts(&write(&dir, "b.gguf", &huge))
            .unwrap()
            .context_length,
        Some(i64::MAX)
    );
}

#[test]
fn gguf_string_typed_context_length_is_skipped_and_parsing_continues() {
    let dir = TempDir::new();
    let gguf = build_gguf(
        3,
        &[
            kv_string("llama.context_length", "8192"),
            kv_string("general.architecture", "llama"),
        ],
    );
    let facts = gguf_facts(&write(&dir, "m.gguf", &gguf)).unwrap();
    assert_eq!(facts.architecture.as_deref(), Some("llama"));
    assert_eq!(facts.context_length, None);
}

#[test]
fn gguf_non_string_architecture_is_skipped_and_template_still_found() {
    let dir = TempDir::new();
    let gguf = build_gguf(
        3,
        &[
            kv_u32("general.architecture", 5),
            kv_string("tokenizer.chat_template", "x"),
        ],
    );
    let facts = gguf_facts(&write(&dir, "m.gguf", &gguf)).unwrap();
    assert_eq!(facts.architecture, None);
    assert!(facts.has_chat_template);
}

#[test]
fn gguf_non_string_chat_template_still_marks_presence() {
    let dir = TempDir::new();
    let gguf = build_gguf(3, &[kv_u32("tokenizer.chat_template", 1)]);
    assert!(
        gguf_facts(&write(&dir, "m.gguf", &gguf))
            .unwrap()
            .has_chat_template
    );
}

#[test]
fn gguf_array_of_strings_is_skipped() {
    let dir = TempDir::new();
    let gguf = build_gguf(
        3,
        &[
            kv_string_array("tokenizer.tokens", &["a", "bb", "ccc"]),
            kv_string("general.architecture", "gemma"),
        ],
    );
    assert_eq!(
        gguf_facts(&write(&dir, "m.gguf", &gguf))
            .unwrap()
            .architecture
            .as_deref(),
        Some("gemma")
    );
}

#[test]
fn gguf_unknown_value_type_aborts_safely() {
    let dir = TempDir::new();
    let gguf = build_gguf(
        3,
        &[
            kv_typed("weird.key", 99, &[]),
            kv_string("general.architecture", "llama"),
        ],
    );
    assert_eq!(
        gguf_facts(&write(&dir, "m.gguf", &gguf))
            .unwrap()
            .architecture,
        None
    );
}

#[test]
fn gguf_truncated_string_value_is_graceful() {
    let dir = TempDir::new();
    let mut gguf = b"GGUF".to_vec();
    gguf.extend_from_slice(&3u32.to_le_bytes());
    gguf.extend_from_slice(&0u64.to_le_bytes());
    gguf.extend_from_slice(&1u64.to_le_bytes());
    gguf.extend(gguf_string("general.architecture"));
    gguf.extend_from_slice(&8u32.to_le_bytes());
    gguf.extend_from_slice(&100u64.to_le_bytes());
    gguf.extend_from_slice(b"abc");
    assert_eq!(
        gguf_facts(&write(&dir, "m.gguf", &gguf))
            .unwrap()
            .architecture,
        None
    );
}

#[test]
fn magic_check_on_short_file_is_false() {
    let dir = TempDir::new();
    let short = write(&dir, "s.gguf", b"GG");
    assert!(!has_gguf_magic(&short));
    assert!(!has_ggml_magic(&short));
}

#[test]
fn safetensors_rejects_zero_and_oversized_and_truncated_headers() {
    let dir = TempDir::new();
    assert_eq!(
        safetensors_header_format(&write(&dir, "z.st", &0u64.to_le_bytes())),
        None
    );

    let oversized = write(&dir, "big.st", &200_000_000u64.to_le_bytes());
    assert_eq!(safetensors_header_format(&oversized), None);

    let mut truncated = 100u64.to_le_bytes().to_vec();
    truncated.extend_from_slice(b"{}");
    assert_eq!(
        safetensors_header_format(&write(&dir, "t.st", &truncated)),
        None
    );
}

#[test]
fn safetensors_non_string_format_is_none() {
    let dir = TempDir::new();
    let file = write(
        &dir,
        "n.st",
        &safetensors_bytes(r#"{"__metadata__":{"format":123}}"#),
    );
    assert_eq!(safetensors_header_format(&file), None);
}
