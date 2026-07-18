//! Tests for `Identification::identify` across its source-kind and file-layout
//! branches.

mod support;

use std::path::Path;

use kernel::records::{Capability, ExecutionMode, Modality, ModelRecord, ModelSource, SourceKind};
use kernel::resolution::{ModelFormat, identify};
use support::TempDir;

fn record(kind: SourceKind, path: &str) -> ModelRecord {
    ModelRecord::new(
        "m",
        Modality::text(),
        Vec::new(),
        ModelSource::new(kind, path),
    )
}

fn write(path: &Path, contents: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn a_builtin_model_has_the_builtin_profile() {
    let id = identify(&record(SourceKind::builtin(), "apple"));
    assert_eq!(id.format, ModelFormat::Builtin);
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.contains(&Capability::chat()));
    assert_eq!(id.execution, ExecutionMode::Stream);
    assert_eq!(id.params.len(), 5);
    assert!(id.params.iter().any(|p| p.key == "temperature"));
}

#[test]
fn an_endpoint_model_has_the_endpoint_profile() {
    let id = identify(&record(SourceKind::endpoint(), "https://api"));
    assert_eq!(id.format, ModelFormat::Endpoint);
    assert_eq!(id.params.len(), 7);
    assert!(id.params.iter().any(|p| p.key == "frequency_penalty"));
    assert!(id.params.iter().any(|p| p.key == "stop"));
}

#[test]
fn an_ollama_chat_model_reads_its_manifest() {
    let dir = TempDir::new();
    let manifest = dir.path().join("manifest");
    write(
        &manifest,
        br#"{"layers":[{"mediaType":"application/vnd.ollama.image.model"}]}"#,
    );
    let mut rec = record(SourceKind::ollama(), manifest.to_str().unwrap());
    // A non-GGUF weight blob → the plain chat profile.
    let blob = dir.path().join("blob");
    write(&blob, b"not-gguf");
    rec.primary_weight_path = Some(blob.to_string_lossy().into_owned());

    let id = identify(&rec);
    assert_eq!(id.format, ModelFormat::OllamaStore);
    assert!(id.capabilities.contains(&Capability::chat()));
    assert!(!id.capabilities.contains(&Capability::see()));
}

#[test]
fn an_ollama_projector_manifest_is_vision() {
    let dir = TempDir::new();
    let manifest = dir.path().join("manifest");
    write(
        &manifest,
        br#"{"layers":[{"mediaType":"x.model"},{"mediaType":"x.projector"}]}"#,
    );
    let id = identify(&record(SourceKind::ollama(), manifest.to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::OllamaStore);
    assert!(id.capabilities.contains(&Capability::see()));
}

#[test]
fn a_ggml_bin_is_transcription() {
    let dir = TempDir::new();
    let bin = dir.path().join("whisper.bin");
    write(&bin, b"lmggDATA"); // GGML magic
    let id = identify(&record(SourceKind::file(), bin.to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::GgmlBin);
    assert_eq!(id.modality, Some(Modality::audio()));
    assert!(id.capabilities.contains(&Capability::transcribe()));
}

#[test]
fn an_mmproj_gguf_is_vision_only() {
    let dir = TempDir::new();
    let gguf = dir.path().join("mmproj-model.gguf");
    write(&gguf, b"GGUF"); // magic; name marks it a projector
    let id = identify(&record(SourceKind::file(), gguf.to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.modality, Some(Modality::vision()));
    assert!(id.capabilities.is_empty());
}

#[test]
fn a_plain_gguf_without_a_known_arch_is_text_chat() {
    let dir = TempDir::new();
    let gguf = dir.path().join("model.gguf");
    write(&gguf, b"GGUF"); // magic but no parseable header → fallback
    let id = identify(&record(SourceKind::file(), gguf.to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.contains(&Capability::chat()));
    assert!(!id.capabilities.contains(&Capability::see()));
}

#[test]
fn a_gguf_beside_an_mmproj_companion_can_see() {
    let dir = TempDir::new();
    write(&dir.path().join("model.gguf"), b"GGUF");
    write(&dir.path().join("mmproj-model.gguf"), b"GGUF");
    let id = identify(&record(
        SourceKind::file(),
        dir.path().join("model.gguf").to_str().unwrap(),
    ));
    assert!(id.capabilities.contains(&Capability::see()));
}

#[test]
fn a_diffusers_model_index_identifies_as_an_image_job() {
    let dir = TempDir::new();
    write(
        &dir.path().join("model_index.json"),
        br#"{"_class_name":"StableDiffusionPipeline"}"#,
    );
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Diffusers);
    assert_eq!(id.execution, ExecutionMode::Job);
    assert_eq!(
        id.pipeline_class.as_deref(),
        Some("StableDiffusionPipeline")
    );
    // The pipeline-family registry resolves the class to an image profile.
    assert_eq!(id.modality, Some(Modality::image()));
    assert!(id.capabilities.contains(&Capability::image()));
    assert!(id.params.iter().any(|spec| spec.key == "steps"));
}

#[test]
fn an_unknown_diffusers_class_falls_back_to_a_bare_job() {
    let dir = TempDir::new();
    write(
        &dir.path().join("model_index.json"),
        br#"{"_class_name":"MysteryPipeline"}"#,
    );
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Diffusers);
    assert_eq!(id.execution, ExecutionMode::Job);
    assert_eq!(id.pipeline_class.as_deref(), Some("MysteryPipeline"));
    assert_eq!(id.modality, None);
    assert!(id.params.is_empty());
}

#[test]
fn a_turbo_scheduler_and_repo_hint_refine_the_params_through_identify() {
    let dir = TempDir::new();
    write(
        &dir.path().join("model_index.json"),
        br#"{"_class_name":"StableDiffusionXLPipeline"}"#,
    );
    write(
        &dir.path().join("scheduler").join("scheduler_config.json"),
        br#"{"_class_name":"EulerAncestralDiscreteScheduler","timestep_spacing":"trailing"}"#,
    );
    // The turbo signal comes from `source.repo` (not the name) — exercising the
    // `repo ?? name` repo-hint fallback.
    let mut rec = record(SourceKind::folder(), dir.path().to_str().unwrap());
    rec.source.repo = Some("stabilityai/sdxl-turbo".to_owned());

    let id = identify(&rec);
    let steps = id.params.iter().find(|s| s.key == "steps").unwrap();
    assert_eq!(
        steps.default_value,
        Some(kernel::records::JsonValue::Int(2))
    );
}

#[test]
fn a_flux_pipeline_without_guidance_embeds_drops_the_guidance_param() {
    let dir = TempDir::new();
    write(
        &dir.path().join("model_index.json"),
        br#"{"_class_name":"FluxPipeline"}"#,
    );
    // No transformer/config.json → guidance is dropped (schnell-style).
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.modality, Some(Modality::image()));
    assert!(!id.params.iter().any(|spec| spec.key == "guidance"));
    assert!(id.params.iter().any(|spec| spec.key == "steps"));

    // With guidance_embeds: true, the guidance param stays (dev-style).
    write(
        &dir.path().join("transformer").join("config.json"),
        br#"{"guidance_embeds":true}"#,
    );
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert!(id.params.iter().any(|spec| spec.key == "guidance"));
}

#[test]
fn a_safetensors_folder_uses_its_config_hint() {
    let dir = TempDir::new();
    write(
        &dir.path().join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"],"max_position_embeddings":8192}"#,
    );
    write(&dir.path().join("model.safetensors"), b"weights");
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Safetensors);
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.contains(&Capability::chat()));
    assert_eq!(id.context_length, Some(8192));
}

#[test]
fn a_sentence_transformers_safetensors_folder_is_embedding() {
    let dir = TempDir::new();
    write(
        &dir.path().join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    );
    write(&dir.path().join("model.safetensors"), b"weights");
    write(&dir.path().join("config_sentence_transformers.json"), b"{}");
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.modality, Some(Modality::embedding()));
    assert_eq!(id.capabilities, vec![Capability::embed()]);
}

#[test]
fn a_config_without_weights_falls_to_unknown_with_the_hint() {
    let dir = TempDir::new();
    // A recognized embedding arch but no safetensors on disk.
    write(
        &dir.path().join("config.json"),
        br#"{"architectures":["BertModel"]}"#,
    );
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Unknown);
    assert_eq!(id.modality, Some(Modality::embedding()));
}

#[test]
fn an_unrecognized_folder_is_unknown() {
    let dir = TempDir::new();
    write(&dir.path().join("README.md"), b"hi");
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Unknown);
    assert_eq!(id.modality, None);
    assert!(id.capabilities.is_empty());
    assert_eq!(id.execution, ExecutionMode::Sync);
}

// A minimal GGUF header builder (mirrors the one in resolution.rs) so identify()
// can exercise the recognized-architecture branch.
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

fn gguf(kvs: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = b"GGUF".to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(&(kvs.len() as u64).to_le_bytes());
    for kv in kvs {
        bytes.extend_from_slice(kv);
    }
    bytes
}

#[test]
fn a_recognized_gguf_architecture_uses_its_profile_and_facts() {
    let dir = TempDir::new();
    // Whisper arch → audio/transcription.
    let whisper = dir.path().join("whisper.gguf");
    write(
        &whisper,
        &gguf(&[kv_string("general.architecture", "whisper")]),
    );
    let id = identify(&record(SourceKind::file(), whisper.to_str().unwrap()));
    assert_eq!(id.modality, Some(Modality::audio()));
    assert!(id.capabilities.contains(&Capability::transcribe()));

    // A llama arch with a context length and chat template → text chat + facts.
    let llama = dir.path().join("llama.gguf");
    write(
        &llama,
        &gguf(&[
            kv_string("general.architecture", "llama"),
            kv_u32("llama.context_length", 4096),
            kv_string("tokenizer.chat_template", "{{ x }}"),
        ]),
    );
    let id = identify(&record(SourceKind::file(), llama.to_str().unwrap()));
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.contains(&Capability::chat()));
    assert_eq!(id.context_length, Some(4096));
    assert_eq!(id.has_chat_template, Some(true));
}

#[test]
fn a_gguf_magic_file_without_the_extension_is_still_gguf() {
    let dir = TempDir::new();
    // No `.gguf` extension, but the GGUF magic → identified by magic.
    let weights = dir.path().join("model.weights");
    write(
        &weights,
        &gguf(&[kv_string("general.architecture", "whisper")]),
    );
    let id = identify(&record(SourceKind::file(), weights.to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.modality, Some(Modality::audio()));
}

#[test]
fn a_missing_snapshot_ref_falls_back_to_the_base_container() {
    let dir = TempDir::new();
    // config.json sits at the base; the ref names a snapshot that isn't present.
    write(
        &dir.path().join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    );
    write(&dir.path().join("model.safetensors"), b"weights");
    let mut rec = record(
        SourceKind::huggingface_cache(),
        dir.path().to_str().unwrap(),
    );
    rec.source.reference = Some("does-not-exist".to_owned());

    let id = identify(&rec);
    assert_eq!(id.format, ModelFormat::Safetensors);
    assert_eq!(id.modality, Some(Modality::text()));
}

#[test]
fn a_hugging_face_record_resolves_its_snapshot_container() {
    let dir = TempDir::new();
    // The config lives under snapshots/<ref>/, not at the base.
    let snapshot = dir.path().join("snapshots").join("rev1");
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["MistralForCausalLM"]}"#,
    );
    write(&snapshot.join("model.safetensors"), b"weights");
    let mut rec = record(
        SourceKind::huggingface_cache(),
        dir.path().to_str().unwrap(),
    );
    rec.source.reference = Some("rev1".to_owned());

    let id = identify(&rec);
    assert_eq!(id.format, ModelFormat::Safetensors);
    assert_eq!(id.modality, Some(Modality::text()));
}
